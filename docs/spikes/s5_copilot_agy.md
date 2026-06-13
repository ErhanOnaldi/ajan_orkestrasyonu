# Spike S5 — `copilot -p` ve `agy -p` davranışı + `--allow-tool` derlemesi

> Faz 0 doğrulama spike'ı. **Throwaway; üretim crate'lerine kopyalanmaz.**
> Spec: `general_plan_and_architecture.md` §3.3, §3.4, §3.5, §5.1 (K8) · `implementation_plan.md` F0.6 / 6.4 / 6.5

| Alan | Değer |
| --- | --- |
| Spike | S5 |
| Hedef araç | copilot + agy |
| Araç sürümü | `copilot 1.0.48` · `agy 1.0.7` |
| Tarih | 2026-06-11 |
| Durum | ✅ tamamlandı |

---

## 1. Amaç

İki kısıtlı adaptör adayını doğrulamak:
- **Copilot (§3.4):** programatik mod stabil mi, `--allow-tool` ince izin filtreleri
  gerçek mi, Policy Engine (K8) bunlara *native* derlenebilir mi, olay normalizasyonu
  nereden yapılacak (stream-json yoksa)?
- **Agy (§3.5):** one-shot mod çalışıyor mu, kritik **oturum kimliği sorunu (issue #7)**
  hâlâ geçerli mi, ID disk/log'dan kurtarılabiliyor mu (geçici çözüm potansiyeli)?

## 2. Komutlar

```bash
# Copilot — metin
copilot -p "Reply with exactly: HELLO_DIVAN" < /dev/null
# Copilot — yazma izni (worktree path'ine daraltılacak olan capability)
copilot -p "Create a file named hello.txt ..." --allow-tool 'write' < /dev/null
# Copilot — kapsamlı shell
copilot -p "Run: echo scoped-ok" --allow-tool 'shell(echo)' < /dev/null

# Agy — metin + issue #7 probe (öncesi/sonrası FS snapshot ile)
agy -p "Reply with exactly: HELLO_DIVAN" --print-timeout 90s < /dev/null

# Agy — follow-up audit: supported olmayan private state yüzeylerini kontrol et
cat ~/.gemini/antigravity-cli/cache/last_conversations.json
find ~/.gemini/antigravity-cli/conversations -maxdepth 1 -type f
grep -E "CLI app data directory|Created conversation|Print mode: conversation=" \
  ~/.gemini/antigravity-cli/log/cli-*.log
```

## 3. Gözlemler

### Copilot (§3.4 doğrulandı — UYGUN, kısıtla)

| Test | Sonuç |
| --- | --- |
| Metin çıktısı | exit 0, stdout = düz metin (**stream-json YOK**, §3.4 teyit) |
| Telemetri | stderr'de `Changes +N -M`, `AI Units`, `Tokens ↑/↓/cached` |
| `--allow-tool 'write'` | dosya prompt'suz yazıldı, exit 0; stderr `Changes +2 -0` |
| `--allow-tool 'shell(echo)'` | komut çalıştı; stderr `● Print scoped-ok (shell)` (insan-okur tool gösterimi) |

- **`--allow-tool` desen sözdizimi doğrulandı:** `write`, `shell(<cmd>)`, `<MCP>(<tool>)`;
  `--deny-tool` ile eşleştirilebilir. Bu **K8'in araç tarafında native uygulanabildiği tek
  adaptör** (§3.4'teki en güçlü artı) — teyit edildi.
  > **EK DOĞRULAMA (Faz 3, 2026-06-12):** `write(<glob>)` **path-scoped** filtresi de
  > çalışıyor: `--allow-tool 'write(/tmp/x/sub/**)'` ile pattern DIŞINA yazma denemesi
  > copilot tarafından REDDEDİLDİ ("permission denied for that path"). Yani CopilotAdapter
  > write yetkisini `write(<worktree>/**)` olarak derler → worktree sınırı araç tarafında
  > gerçekten zorlanır (F3.4), yalnız `current_dir` değil.
- **Olay normalizasyonu:** stream-json olmadığı için `FileEdit` worktree git diff'inden
  türetilir (stderr `Changes +N -M` yalnız kaba sayaç, path vermez). `ToolCall` granülaritesi
  stderr'deki `●` satırlarından kısmen okunabilir ama v1'de eksik kabul edilir (§3.4).

### Agy (§3.5 doğrulandı — KISITLI, degraded one-shot)

| Test | Sonuç |
| --- | --- |
| Metin çıktısı | exit 0, stdout = düz metin, stderr boş (**stream-json YOK**) |
| Çıktıda conversation/session id | **YOK** (stdout & stderr) |
| CLI app data dir | `~/.gemini/antigravity-cli` |
| Private cache/log ID | **VAR**: `cache/last_conversations.json` ve `log/cli-*.log` içinde konuşma UUID'si görülebiliyor |
| Konuşma store'u | **VAR**: `conversations/<uuid>.db` SQLite store'u oluşuyor |

- **issue #7 ampirik olarak yeniden üretildi:** `agy -p` oluşturduğu conversation ID'yi
  stdout/stderr üzerinden desteklenen bir makine-okur sözleşmeyle döndürmüyor. GitHub issue
  #7 hâlâ bu yüzeyi talep ediyor.
- **Disk/log üzerinden geçici ID yakalama mümkün ama v1 için güvenilir sözleşme değil:**
  `~/.gemini/antigravity-cli/cache/last_conversations.json` cwd → UUID eşlemesi tutuyor;
  CLI log'u da `Created conversation <uuid>` ve `Print mode: conversation=<uuid>` satırları
  yazıyor. Bu yüzeyler dokümante edilmiş adaptör API'si değil, cwd/global state'e bağlı ve
  paralel orkestrasyonda race/cross-contamination riski taşıyor.
- Bu nedenle tam multi-turn AgyAdapter v1'e alınmaz. `-c/--continue` global son konuşmayı,
  `--conversation <id>` belirli ID'yi devam ettiriyor; ancak Divan v1 desteklenen olmayan
  cache/log scraping'e dayanmayacak.

## 4. Ham çıktı konumu

`docs/spikes/raw/s5_copilot_agy.txt` (secret yok; FS path'leri lokal).

## 5. Karar + §3.3 matris güncelleme önerisi

**Copilot = Faz 3'te 3. adaptör (UYGUN, kısıtla).** `--allow-tool` ↔ Policy Engine derlemesi
gösterim senaryosudur (F3.4). **Agy = degraded one-shot adaptör (`review/research/analyze/
report`), `resume=Unsupported`, `multi_turn=0`, router multi-turn'de exclude.** Upstream
issue #7 çözülene veya Google desteklenen bir `--print` metadata/ID sözleşmesi sunana kadar
tam adaptöre yükseltme ertelenir. Private cache/log scraping v1 kapsamına alınmaz.

§3.3 matrisi — **doğrulanmış sürüm** (S1–S5 sonrası; öneri):

| Araç | Spawn | Akış çıktısı | Oturum devamı | Uygunluk (doğrulanmış) |
| --- | --- | --- | --- | --- |
| Claude Code `2.1.173` | `claude -p` | `stream-json` ✅ (S1) | `-r <session_id>` ✅ | **TAM** — referans |
| Codex `0.139.0` | `codex exec` | `--json` JSONL ✅ (S2) | `exec resume <thread_id>` ✅ | **TAM** |
| Copilot `1.0.48` | `copilot -p` | düz metin ⚠️ (S5) | var; v1'de tek-atış öncelikli | **UYGUN (kısıtla)** — FileEdit diff'ten |
| Antigravity `agy 1.0.7` | `agy -p` | düz metin ⚠️ (S5) | desteklenen ID çıktısı yok ❌; private cache/log ID var ama v1'de kullanılmayacak | **KISITLI** — degraded one-shot |
| OpenCode `1.14.50` | stdin modu | (doğrulanmadı) | var | **TAM (doğrulanacak)** — S5 dışı |

> Not: `codex e` alias hâlâ geçerli; tam form `codex exec`. Sürümler 2026-06-11 taramasından.

## 6. Üretim etkisi

- **CopilotAdapter (F3.4):** Policy → `--allow-tool` compiler: `write` capability → yalnız
  task worktree path; shell allowlist → task policy; URL/MCP → config. `FileEdit` worktree
  git diff'ten; `ToolCall` granülaritesi eksikse trace'e `tool_call_unavailable` metadata.
  Teslim yolu önceliği: MCP > idle-resume (§3.4).
- **AgyAdapter (F3.5):** yalnız one-shot kind'lar; `resume()` → `Unsupported` (Card'da beyan);
  Cost Router kuralı `match:{multi_turn:true} exclude:{tool:agy}`. Faz 0/S5 issue #7
  kontrolü: **hâlâ açık; yalnız unsupported cache/log workaround var** → degraded tasarım
  korunur.
- **Ortak:** Copilot/Agy düz-metin adaptörleri `NormalizedEvent`'i akıştan değil
  (a) nihai stdout, (b) worktree diff, (c) exit code'dan türetir — S1/S2'nin JSONL
  parser'ından ayrı bir "text+diff" normalize yolu gerekir (`divan-adapters` iki strateji).
