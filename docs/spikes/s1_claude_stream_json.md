# Spike S1 — `claude -p --output-format stream-json` parse + event normalize

> Faz 0 doğrulama spike'ı. **Throwaway; üretim crate'lerine kopyalanmaz.**
> Spec: `general_plan_and_architecture.md` §3.2 (olay normalizasyonu), §3.3 · `implementation_plan.md` F0.2 / 6.2

| Alan | Değer |
| --- | --- |
| Spike | S1 |
| Hedef araç | claude (referans adaptör) |
| Araç sürümü | `2.1.173 (Claude Code)` |
| Tarih | 2026-06-11 |
| Durum | ✅ tamamlandı |

---

## 1. Amaç

ClaudeAdapter'ın temelini doğrulamak: `claude -p` headless modu yapılandırılmış
`stream-json` üretiyor mu, hangi olay türleri var, `NormalizedEvent` (§3.2:
`ToolCall`, `FileEdit`, `TurnEnd`, `SessionIdle`, `SessionEnd`, `Error`) bu akıştan
deterministik türetilebilir mi, resume için `session_id` yakalanıyor mu? (K1: zarf
A2A'dan, ama Claude TAM uyumlu referans adaptördür.)

## 2. Komutlar

```bash
# Metin-only tur (event iskeleti + session_id + result)
claude -p "Reply with exactly the text: HELLO_DIVAN. Do not use any tools." \
  --output-format stream-json --verbose --max-turns 1

# Araç kullanan tur (tool_use + tool_result yakalamak için, sandbox dizinde)
cd /tmp/divan_s1_tool && claude -p "Create a file named hello.txt ... Use the Write tool." \
  --output-format stream-json --verbose --max-turns 4 --permission-mode acceptEdits
```

Notlar:
- `stream-json` **yalnız `--print/-p` ile** çalışır ve `--verbose` zorunludur (aksi halde
  CLI reddeder).
- stdin bağlı değilse CLI 3 sn sonra uyarı verip devam eder; adaptör spawn'da
  `stdin < /dev/null` vermelidir (gereksiz 3 sn gecikmeyi önler).
- Araç çalıştırmak için `--permission-mode acceptEdits` (veya `bypassPermissions`)
  gerekti; yoksa Write tool onay bekleyip headless'ta ilerlemez.

## 3. Gözlemler

Akış = **satır-başına-bir JSON nesnesi (JSONL)**. Gözlemlenen üst düzey `type` değerleri:

| `type` | `subtype` | Anlamı | Taşıdığı kilit alanlar |
| --- | --- | --- | --- |
| `system` | `init` | Oturum başlangıcı | `session_id`, `model`, `cwd`, `tools[]`, `mcp_servers[]`, `permissionMode`, `slash_commands` |
| `system` | `post_turn_summary` | **Tur sınırı işareti** | `needs_action`, `status_category`, `status_detail`, `summarizes_uuid` |
| `assistant` | — | Model mesajı | `message.content[]`: `text` ve/veya `tool_use` blokları |
| `user` | — | Araç sonucu enjeksiyonu | `message.content[]`: `tool_result` (`is_error`, `content`) |
| `rate_limit_event` | — | Kota/limit telemetrisi | `rate_limit_info` |
| `result` | `success` / `error_*` | **Oturum sonu** | `is_error`, `num_turns`, `duration_ms`, `usage{input,output,cache_*}`, `result` (özet metin) |

Doğrulanan gerçekler:
- `session_id` ilk `system/init` satırında gelir ve tüm satırlarda tekrarlanır →
  resume için güvenilir yakalanabilir.
- `tool_use` bloğu `name` + `input` taşır; `Write`/`Edit` için `input.file_path` mevcut →
  `FileEdit` doğrudan türetilir.
- `result.subtype` başarı/hata sınıfını verir (`success`, `error_max_turns`, vb.);
  `is_error` bool bayrağı oturum sonucu için kesin.
- `post_turn_summary` tur bittiğinde gelir → hook-bağımsız **TurnEnd sinyali** olarak
  kullanılabilir (Faz 2 batching tur sınırı buna denk gelir).

## 4. Ham çıktı konumu

`docs/spikes/raw/s1_claude_stream_json.txt` (sanitize: secret/token yok; session UUID
ephemeral lokal değer, zararsız). MCP server listesi kullanıcının claude.ai
bağlayıcılarıdır — Divan ile ilgisizdir, adaptör bunları yok sayar.

## 5. Karar

**Claude = TAM uyumlu referans adaptör (§3.3 doğrulandı).** Olay normalizasyonu
**akıştan deterministik** yapılabilir; diff-tabanlı türetmeye gerek yok. ClaudeAdapter
`stream-json` JSONL parser'ı kullanır.

`NormalizedEvent` ↔ claude akışı eşlemesi (üretim taslağı):

| `NormalizedEvent` | Claude kaynağı |
| --- | --- |
| `SessionStart{session_id, model}` | `system/init` |
| `ToolCall{name, input}` | `assistant` → `content[].tool_use` |
| `FileEdit{path}` | `tool_use` where `name ∈ {Write, Edit, MultiEdit, NotebookEdit}` → `input.file_path` |
| `TurnEnd` | `system/post_turn_summary` |
| `SessionEnd{ok, usage}` | `result` (`is_error`, `usage`) |
| `Error{Retryable}` | `rate_limit_event`; `result.subtype ∈ {error_max_turns, error_during_execution}` (geçici) |
| `Error{Fatal}` | parse çöküşü; `is_error=true` + non-retryable subtype; CLI exit ≠ 0 |
| `SessionIdle` | akıştan **gelmez** → Faz 2 hook (Stop/idle) ile sağlanır (bkz. S3) |

## 6. Üretim etkisi (Faz 1 ClaudeAdapter)

- Spawn: `claude -p <prompt> --output-format stream-json --verbose --max-turns N
  --permission-mode acceptEdits`, `cwd=worktree`, `stdin < /dev/null`.
- Observe: JSONL satır satır parse; bilinmeyen `type` → `tool_call_unavailable`
  benzeri trace metadata ile yut, akışı kırma.
- Resume: `claude -p -r <session_id> <prompt> ...` (session_id init'ten alınır).
- Parse hatası: ham satır artifact/log'a yazılır, `Error{Fatal}` üretilir (taksonomi 6.1).
- `SessionIdle` akışta yok → idle-wake yalnız hook yoluyla (S3); Card'da beyan edilir.
- **S2 ile ortak `NormalizedEvent` modeli** `s2_codex_json.md` §5'te birleştirilecek.
