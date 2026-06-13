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
| F3.6 | Demo (policy denial + router decision + 3. adaptör) | ✅ test kapsaması; canlı opsiyonel |

**Test durumu:** `cargo test --workspace` → 147 test geçiyor. `cargo fmt` + `cargo clippy
--workspace --all-targets` temiz.

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
  derlemesi (`write` capability → native `write` filtresi, worktree'ye
  `current_dir` ile sınırlı). Düz metin çıktı → `FileEdit` git diff'ten türetilir;
  `ToolCall` granülaritesi yok (dokümante, sentetik `tool_call_unavailable` not).
- **AgyAdapter** (degraded, §3.5): `agy -p` one-shot; `multi_turn=false` (router
  multi-turn işlerde dışlar); `resume` her zaman `Unsupported` (issue #7); kabul
  edilen kind'lar `review/research/analyze/report`.
- 4 adaptör de `divan agents` / AgentCard listesinde.

## F3.6 demo kapsaması (otomatik)

- `policy::tests` — yetkisiz write/kill reddi + trace (F3.1).
- `messaging_rpc.rs` — `pretooluse_enforces_worktree_write_boundary` (F3.2),
  `kill_session_denied_for_non_owner` (F3.6), `delegate_task_is_policy_gated`.
- `router::tests` — review→farklı vendor, multi-turn→agy hariç, no-candidate.
- Canlı demo (gerçek claude/copilot writer + farklı vendor reviewer + worktree
  dışı yazma reddi) `~/.claude` hook kurulumu + token gerektirir; kullanıcı tetikler.

## Bilinen sınırlar (Faz 3)

- Path canonicalization lexical (symlink kaçışı sertleştirmesi sonraki iş).
- Router skoru sabit (telemetri-beslemeli skor v1.5).
- Copilot/Agy resume sınırlı (copilot doğrulanmadı; agy issue #7).

## Faz 4'e devir

`divan tui` (ratatui), `divan trace <id>` timeline, token/maliyet metrikleri.
