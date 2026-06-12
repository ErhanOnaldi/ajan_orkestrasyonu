# Divan — Faz 1 Durumu ve Bilinen Sınırlar

Kaynak: `general_plan_and_architecture.md` §7 (Faz 1) · `implementation_plan.md` §9 (F1.x).
Tarih: 2026-06-11.

## Tamamlanan iş paketleri

| Paket | Konu | Durum |
| --- | --- | --- |
| F1.1 | Cargo workspace + core/db/daemon/cli/adapters/trace crate'leri | ✅ |
| F1.2 | SQLite migration (`0001_initial.sql`) + store/repository katmanı | ✅ |
| F1.3 | Daemon + unix-socket JSON-RPC, lock file, startup reconciliation, watchdog | ✅ |
| F1.4 | Content-addressed Artifact Store (BLAKE3, dedupe, traversal koruması) | ✅ |
| F1.5 | Task Scheduler v1 + write-review flow + `Error{retryable\|fatal}` retry | ✅ |
| F1.6 | Worktree Manager v1 (`create/diff/merge/cleanup`, `is_dirty`) | ✅ |
| F1.7 | ClaudeAdapter (S1 stream-json parser + spawn/observe/resume/kill) | ✅ |
| F1.8 | CodexAdapter (S2 exec --json parser; exit-based SessionEnd) | ✅ |
| F1.9 | CLI: `up/down/status/run/log/diff/merge/cleanup` | ✅ |
| F1.10 | Demo + bu sınırlar belgesi | ⏳ canlı demo GIF kullanıcıda; fake-adaptör e2e testleri ✅ |

**Test durumu:** `cargo test --workspace` → 85 test geçiyor. `cargo fmt` + `cargo clippy
--workspace --all-targets` temiz. Gerçek-CLI lifecycle dumanı (`up/status/down`) doğrulandı.

## Doğrulanan kabul kriterleri (fake adaptör ile)

- `implement → review → done` akışı uçtan uca (`write_review_happy_path_to_done`).
- Başarısız implement → review başlamaz (`failed_implement_blocks_review`).
- **`ok=false` oturumu `done` değil `failed` olur** (`session_ok_false_marks_task_failed_not_done`).
- **Retryable hata sınırlı backoff ile yeniden denenir** (`retryable_errors_are_retried_then_succeed`)
  ve tükenince `failed` olur (`retryable_errors_exhaust_then_fail`).
- **Review artifact reviewer'ın gerçek çıktısını taşır** (`review_artifact_contains_reviewers_final_text`).
- **Commit edilmemiş worktree düzenlemeleri diff'e yansır** (`commit_all_captures_uncommitted_edits_*`).
- Takılan oturum watchdog ile `failed(timeout)` (`hanging_session_times_out_to_failed`).
- Startup reconciliation öksüz task'ı `failed(orphaned)` yapar (`reconcile_orphans_*`).
- İkinci daemon başlatma reddi (lock file) (`lock_blocks_second_acquire_while_held`).
- State transition + trace event aynı transaction'da (`legal_transition_*_atomically`).
- Artifact dedupe + path traversal reddi (F1.4 testleri).
- Gerçek-CLI kontrat testleri `DIVAN_TEST_CLAUDE=1` / `DIVAN_TEST_CODEX=1` arkasında
  (`real_claude_*`, `real_codex_*`; CI'da kapalı, varsayılan erken döner).

## Review-yolu düzeltmeleri (kod incelemesi sonrası, 2026-06-11)

Bir kod incelemesi gerçek write-review yolunda altı sorun bulmuştu; hepsi giderildi:

1. **Worktree diff/merge commit edilmemiş düzenlemeleri atlıyordu** → implement sonrası
   worktree otomatik commit'lenir (`WorktreeManager::commit_all`, tüm değişiklikler +
   untracked), böylece üç-nokta diff ve merge gerçek içerik üzerinde çalışır.
2. **Review artifact gerçek incelemeyi taşımıyordu** (read-only sandbox `.divan-review.md`
   yazamaz) → reviewer'ın nihai metni `SessionEnd.final_text` ile yakalanır (claude
   `result.result`, codex son `agent_message`) ve review artifact'ı olur.
3. **`ok=false` oturum `done` olabiliyordu** → `run_session_once` artık `ok=false`'u
   `Fatal` sayar; task `failed` olur, review başlamaz.
4. **Implement prompt'u spec'i kaybediyordu** (yalnız 60-karakter title) → tam spec
   artifact içeriği prompt'a gömülür (Faz 2 MCP retrieval'a kadar).
5. **CLI final report ref'ini göstermiyordu** → `FinalReport.final_report_ref` eklendi,
   CLI yazdırır (F1.9 gereği).
6. **Gerçek-CLI / retry testleri eksikti** → env-gated `real_claude`/`real_codex` ve
   retry/exhaust testleri eklendi.

## Bilinen sınırlar (v1 — bilinçli kararlar)

- **Gerçek-CLI write-review demosu (F1.10) henüz canlı koşturulmadı.** Tüm akış fake
  adaptörle test edildi; `divan run ... --repo <gerçek repo>` gerçek `claude`/`codex`
  çağırır (token harcar, dakikalar sürebilir). Komut hazır, demo kullanıcı tarafından
  tetiklenecek.
- **Run kayıtları bellekte.** `divan diff|merge|cleanup <task-id>` yalnız daemon yeniden
  başlatılmadan çalışır (worktree run map'i RAM'de). `.divan/runs/` kalıcılığı sonraki iş.
- **Artifact GC yok.** `divan cleanup` yalnız worktree siler; `.divan/artifacts/` büyür
  (spec §3.7; `divan gc` v2).
- **Mesaj fan-out / hook / MCP teslimi Faz 2.** Şema (`origin_message_id`) hazır; teslim
  yolu (hook injection, MCP server) henüz yok. `messages` yalnız pointer doğrulamalı yazılır.
- **Policy Engine / Cost Router Faz 3.** Şu an writer=claude-1, reviewer=codex-1 sabit.
- **TUI / trace timeline Faz 4.** `divan log` düz metin event listesi verir.
- **Re-attach yok.** Daemon ölümünde hayatta kalan oturumlar reconciliation'da öksüz
  sayılır (spec §3.7; re-attach v2).
- **`SessionIdle` akıştan gelmez.** Idle-wake hook yolu Faz 2 (spike S1/S2 §5).
- **Platform:** macOS + Linux (unix socket). Windows v2 (P0.4).

## Sonraki adımlar (Faz 2 devri)

- `divan install-hooks` (Claude/Codex), tur-arası enjeksiyon, idle wake.
- MCP server yüzü (`send_message`, `delegate_task`, …) policy kontrolünden geçerek.
- Batching (K5) + subscription fan-out (K6) + delivery receipts.
- `file_touches` 30 sn çakışma penceresi (şema hazır; tetikleme Faz 2).
