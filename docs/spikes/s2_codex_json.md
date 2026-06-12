# Spike S2 — `codex exec --json` event modeli + ortak `NormalizedEvent`

> Faz 0 doğrulama spike'ı. **Throwaway; üretim crate'lerine kopyalanmaz.**
> Spec: `general_plan_and_architecture.md` §3.2, §3.3 · `implementation_plan.md` F0.3 / 6.3

| Alan | Değer |
| --- | --- |
| Spike | S2 |
| Hedef araç | codex |
| Araç sürümü | `codex-cli 0.139.0` |
| Tarih | 2026-06-11 |
| Durum | ✅ tamamlandı |

---

## 1. Amaç

CodexAdapter temelini doğrulamak ve **S1 ile ortak `NormalizedEvent` modelini**
çıkarmak: `codex exec --json` yapılandırılmış JSONL üretiyor mu, olay modeli claude'dan
ne kadar farklı, `ToolCall`/`FileEdit`/`SessionEnd` türetilebilir mi, resume kimliği
yakalanıyor mu?

## 2. Komutlar

```bash
# Metin-only tur
codex exec --json --skip-git-repo-check --sandbox read-only -C <dir> \
  "Reply with exactly the text: HELLO_DIVAN" < /dev/null

# Yazma turu — sandbox AÇIK kalır (workspace-write), yalnız onay prompt'u bastırılır
codex exec --json --skip-git-repo-check --sandbox workspace-write -C <dir> \
  -c approval_policy=never \
  "Create a file named hello.txt containing exactly one line: divan" < /dev/null
```

Notlar:
- Modern CLI'da subcommand `exec` (`e` alias hâlâ geçerli). Spec §3.3'teki `codex e`
  doğrulandı; tam form `codex exec`.
- **stdin `< /dev/null` zorunlu**: aksi halde `exec` "Reading additional input from
  stdin..." ile bloke olur (S2 ilk denemesi bu yüzden takıldı — claude ile aynı tuzak).
- `--skip-git-repo-check`: worktree/temp dizinlerde git reposu yoksa gerekli.
- **Güvenlik kararı:** `--dangerously-bypass-approvals-and-sandbox` KULLANILMAZ
  (sandbox'ı kapatır). Üretimde de adaptör `--sandbox workspace-write` +
  `-c approval_policy=never` kullanmalı: sandbox açık, prompt yok. Policy Engine (K8)
  worktree sınırını sandbox kökü olarak verir.

## 3. Gözlemler

> **DÜZELTME (2026-06-12, Faz 1 canlı demo):** Bu rapordaki ham örnekler
> `jq` ile *düzleştirilmiş* (projeksiyon) idi; gerçek wire formatı `item`
> alanlarını **`item` nesnesi altında nest'ler**. Doğru yapı:
> `{"type":"item.completed","item":{"type":"agent_message","text":"…"}}` —
> yani ayrımcı `item.type`, metin `item.text`, değişiklikler `item.changes[]`.
> `thread_id` ve `usage` üst düzeydedir (değişmedi). CodexAdapter parser'ı bu
> nested yapıya göre düzeltildi; `agent_message` metni `SessionEnd.final_text`
> olarak yakalanıp review artifact'ı olur. Test fixture'ları gerçek şekle güncellendi.

Codex olay modeli claude'dan **yapısal olarak farklı**: hiyerarşi
**thread → turn → item**. Akış yine JSONL.

| `type` | Anlamı | Kilit alanlar |
| --- | --- | --- |
| `thread.started` | Oturum başlangıcı | `thread_id` (**resume anahtarı**) |
| `turn.started` | Tur başlangıcı | — |
| `item.completed` | Atomik iş öğesi | `item.type` + türe özel alanlar |
| `turn.completed` | Tur sonu | `usage{input,cached_input,output,reasoning_output}` |

Gözlemlenen `item.type` değerleri:

| `item.type` | Alanlar | Eşlenir |
| --- | --- | --- |
| `agent_message` | `text` | assistant metin |
| `command_execution` | `command`, `exit_code`, `status` (`completed`/`failed`), `aggregated_output` | `ToolCall` (shell) |
| `file_change` | `changes[]` = `{path, kind: add\|modify\|delete}`, `status` | **`FileEdit` (native!)** |

Doğrulanan gerçekler:
- `thread_id` `thread.started`'ta gelir → resume için yakalanır (`codex exec resume <id>`).
- **`file_change` olayı path + kind taşır** → claude gibi codex de FileEdit'i diff'ten
  türetmeye gerek bırakmaz (kontrast: Copilot, S5'te diff-derived olacak).
- `command_execution.exit_code` + `status` → araç hatası sınıflandırması için yeterli
  sinyal.
- `turn.completed.usage` cached token ayrımı dâhil maliyet telemetrisi verir.

## 4. Ham çıktı konumu

`docs/spikes/raw/s2_codex_json.txt` (sanitize: secret yok; `thread_id` ephemeral lokal).

## 5. Karar + Ortak `NormalizedEvent` modeli (S1+S2 sentezi)

**Codex = TAM uyumlu adaptör (§3.3 doğrulandı).** İki araç da JSONL + native file-change
verdiği için çekirdek normalize katmanı her ikisini de akıştan besler.

Birleşik `NormalizedEvent` (üretim taslağı — `divan-core`):

| `NormalizedEvent` | Claude (S1) | Codex (S2) |
| --- | --- | --- |
| `SessionStart{id, model}` | `system/init` (`session_id`) | `thread.started` (`thread_id`) |
| `ToolCall{name, input}` | `assistant.tool_use` | `item:command_execution` / `mcp_tool_call` |
| `FileEdit{path, kind}` | `tool_use{Write/Edit}.file_path` (kind=heuristik) | `item:file_change.changes[]` (`kind` native) |
| `TurnEnd` | `system/post_turn_summary` | `turn.completed` |
| `SessionEnd{ok, usage}` | `result` (`is_error`, `usage`) | `turn.completed.usage` + process exit |
| `Error{Retryable}` | `rate_limit_event`; `result.subtype error_*` | `command_execution.status=failed` (geçici); exit-code rate limit |
| `Error{Fatal}` | parse çöküşü; non-retryable subtype; exit≠0 | parse çöküşü; auth/login eksik; exit≠0 |
| `SessionIdle` | akıştan gelmez → hook (S3) | akıştan gelmez → MCP/resume yolu |

**Tasarım sonucu:** `divan-core::NormalizedEvent` enum'u yukarıdaki 7 varyantla
sabitlenir. Her adaptör kendi JSONL'ini bu enum'a map eder; çekirdek yalnız enum'u görür
(§3.2 "Hub yalnızca normalize olay görür"). `kind`/`model`/`usage` opsiyonel alanlar
adaptör-spesifik zenginleştirmedir.

## 6. Üretim etkisi (Faz 1 CodexAdapter + core)

- `divan-core`: `NormalizedEvent` enum'u bu 7 varyantla tanımlanır; `AgentErrorClass`
  `{Retryable, Fatal}`.
- CodexAdapter spawn: `codex exec --json --skip-git-repo-check --sandbox workspace-write
  -C <worktree> -c approval_policy=never <prompt> < /dev/null`.
- Review (salt okunur) görevleri: `--sandbox read-only`.
- Resume: `codex exec resume <thread_id> ...` (thread_id `thread.started`'tan).
- Hem claude hem codex parser'ı **ortak satır-bazlı JSONL okuyucu** + araç-spesifik
  `map_event()` ile yazılır; bilinmeyen `type`/`item.type` yutulur, akış kırılmaz.
- **Güvenlik kuralı (yeni, ADR'ye aday):** Hiçbir adaptör sandbox-bypass bayrağı
  kullanmaz; izolasyon sandbox + worktree path ile sağlanır.
