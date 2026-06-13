# Divan — Faz 3 Durumu: İzin, Maliyet, 3. Adaptör

Kaynak: `general_plan_and_architecture.md` §5.1/§5.2/§3.4/§3.5 · `implementation_plan.md` §F3.
Tarih: 2026-06-12.

## Tamamlanan iş paketleri

| Paket | Konu | Durum |
| --- | --- | --- |
| F3.1 | Policy Engine core (capability matrix, path canonicalization, worktree boundary) | ✅ |
| F3.2 | Hook PreToolUse write-path kontrolü | ✅ |
| F3.3 | Router YAML engine (match/prefer/exclude, `different_vendor_than`, `multi_turn` exclude) | ✅ |
| F3.4 | CopilotAdapter (`--allow-tool` derlemesi, diff-tabanlı FileEdit) | ✅ |
| F3.5 | AgyAdapter (degraded one-shot, `multi_turn=false`, resume=Unsupported) | ✅ |
| F3.6 | Demo (policy denial + router decision + 3. adaptör) | ✅ canlı koşuldu (2026-06-12) |

**Test durumu:** `cargo test --workspace` → **149 test** geçiyor (adapters 30, daemon-lib 45,
messaging_rpc 6, core 26, db 31, hooks 10, trace 1). `cargo fmt` + `cargo clippy
--workspace --all-targets` temiz. Not: 4 gerçek-CLI testi (`DIVAN_TEST_CLAUDE/CODEX/
COPILOT/AGY`) env-gated — env yoksa erken döner (yine "passed"), varsa gerçek CLI'ı çağırır.

## Policy Engine (K8, §5.1)

İki katman (`crates/divan-daemon/src/policy.rs`):
- **MCP tool chokepoint** (`check`): her MCP tool handler buradan geçer (F2.3).
- **Execution-action matrix** (§5.4): `check_write` (write capability + path
  worktree içinde; lexical canonicalization `..` kaçışını engeller), `check_kill`
  (kill capability + oturum sahipliği), `check_spawn` (cost_class limiti). Her red
  who/what/why `policy_denied` trace üretir.
- Bilinen sınır: lexical path normalization symlink kaçışını çözmez (worktree
  içine kötü niyetli symlink dışarı işaret edebilir) — Faz sonrası sertleştirme.

**F3.2 PreToolUse:** `divan install-hooks` artık bir `PreToolUse` hook'u
(`Write|Edit|MultiEdit` matcher) kurar; tool çalışmadan önce daemon
`hook.pretooluse` ajanın aktif worktree'sine karşı `check_write` çalıştırır,
worktree dışı yazma `permissionDecision: deny` ile engellenir (ajan oturumu
kırılmadan devam eder).

## Cost Router (K9, §5.2)

`crates/divan-daemon/src/router.rs` — deterministik YAML kural motoru
(`examples/router.rules.yaml`). `match`/`prefer`/`exclude`, `cost_class`
kısıtları (`<=`, `>=`, `=`), `different_vendor_than: author`, `multi_turn`
exclude. Aday yoksa deterministik `NoCandidate` hatası.
- write-review akışı reviewer'ı router ile seçer: writer'dan **farklı vendor**
  (`router_decision` trace).
- `divan router explain <task-id>`: seçilen ajan + eşleşen kural id'leri +
  gerekçeler; Copilot seçilirse derlenen `--allow-tool` argümanları (F3.4).

## Adaptörler

- **CopilotAdapter** (3. adaptör, §3.4): `copilot -p` + `--allow-tool`
  derlemesi. Write yetkisi **`write(<worktree>/**)`** path-scoped filtresine
  derlenir; **canlı doğrulandı (2026-06-12):** copilot pattern dışına yazmayı
  reddeder ("permission denied for that path") — worktree sınırı araç tarafında
  gerçekten zorlanır (yalnız `current_dir` değil). Düz metin çıktı → `FileEdit`
  git diff'ten; `ToolCall` granülaritesi yok (sentetik `tool_call_unavailable`).
- **AgyAdapter** (degraded, §3.5): `agy -p` one-shot; `multi_turn=false` (router
  multi-turn işlerde dışlar); `resume` her zaman `Unsupported` (issue #7); kabul
  edilen kind'lar `review/research/analyze/report`.
- 4 adaptör de `divan agents` / AgentCard listesinde.

## F3.6 demo — otomatik kapsama

- `policy::tests` — yetkisiz write/kill reddi + trace (F3.1).
- `messaging_rpc.rs` — `pretooluse_enforces_worktree_write_boundary` (F3.2),
  `kill_session_denied_for_non_owner` (F3.6), `delegate_task_is_policy_gated`.
- `router::tests` — review→farklı vendor, multi-turn→agy hariç, no-candidate,
  tiebreak full-adapter > degraded one-shot.

## F3.6 demo — canlı koşum (2026-06-12, gerçek CLI)

Gerçek `claude` + `codex` ile, throwaway repo'da koşuldu. Doğrulananlar:

- **Yetkisiz kill reddi (F3.6):** soket üzerinden `kill_session{caller:codex-1,
  owner:claude-1}` → `policy denied: codex-1 cannot kill: lacks kill capability`.
- **Copilot worktree write sınırı (F3.4):** `copilot --allow-tool 'write(<glob>)'`
  ile pattern dışına yazma denemesi copilot tarafından REDDEDİLDİ ("permission
  denied for that path") → CopilotAdapter `write(<worktree>/**)` derler (yalnız
  `current_dir` değil; araç tarafında gerçek sınır).
- **Router cross-vendor reviewer (F3.3):** `divan run "<görev>" --flow write-review`
  → writer claude-1, router reviewer'ı **codex-1** seçti (writer'dan farklı vendor);
  `router_decision` trace + `divan router explain <review-task>` kararı + eşleşen
  kural [1] + gerekçeleri gösterdi; akış temiz tamamlandı (4 artifact).
- **Canlı demoda yakalanan gerçek bulgu:** ilk koşumda router eşit-skorlu review
  adaylarında en ucuzu (agy, degraded one-shot, issue #7) seçti; agy takıldı ve
  900s review watchdog'u `failed(timeout)` ile düşürdü (routing + watchdog doğru
  çalıştı). Tiebreak tam multi-turn adaptörü tercih edecek şekilde düzeltildi →
  reviewer codex-1, temiz koşum.

Not: tam hook-injection canlı demosu (`~/.claude`'a PreToolUse hook kurulumu)
hâlâ kullanıcı tetikler; F3.2 mekanizması daemon RPC + `/bin/sh` testleriyle örtülü.

## Bilinen sınırlar (Faz 3)

- Path canonicalization lexical (symlink kaçışı sertleştirmesi sonraki iş).
- Router skoru sabit (telemetri-beslemeli skor v1.5).
- Copilot/Agy resume sınırlı (copilot doğrulanmadı; agy issue #7).
- `check_spawn` bir **primitif**: ajan-başlatımlı spawn (capability `spawn`) için
  hazır; Faz 3'te ajan-yüzlü spawn giriş noktası yok (hub'ın kendi write-review
  spawn'ları hub yetkisidir, watchdog gibi). Sub-agent/spawn MCP tool'u eklendiğinde
  o handler `check_spawn`'ı çağırır.

## Faz 4'e devir

`divan tui` (ratatui), `divan trace <id>` timeline, token/maliyet metrikleri.
