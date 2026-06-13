# Divan — Faz 2 Durumu: Mesajlaşma Çekirdeği

Kaynak: `general_plan_and_architecture.md` §3.6/§5.3 · `implementation_plan.md` §F2.
Tarih: 2026-06-12.

## Tamamlanan iş paketleri

| Paket | Konu | Durum |
| --- | --- | --- |
| F2.1 | Hook installer (`divan install-hooks`, merge/backup, no-op-when-down) | ✅ |
| F2.2 | Activity + turn-boundary RPC (`hook.activity/turn_end/session_idle/confirm`) | ✅ |
| F2.3 | MCP server (`divan-mcp`, rmcp 1.7, 8 araç) + daemon tool yüzleri | ✅ |
| F2.4 | Subscription engine (fan-out, `origin_message_id`, broadcast capability gate) | ✅ |
| F2.5 | Batching + delivery receipts (peek→inject→confirm; pending-on-failure) | ✅ |
| F2.6 | Conflict detection (`file_touches`, 30 sn pencere → pointer alert) | ✅ |
| F2.7 | Demo (aşağıdaki prosedür + otomatik kapsama) | ✅ test kapsaması; canlı opsiyonel |

**Test durumu:** `cargo test --workspace` → 121 test geçiyor. `cargo fmt` + `cargo clippy
--workspace --all-targets` temiz.

## Mimari (K2–K6)

- **Deterministik çekirdek (K2):** `divan-daemon::bus::MessageBus` — sıfır LLM. Eşleştirme,
  fan-out, kuyruk, batch, çakışma hepsi kodda.
- **Push, poll değil (K3):** ajan "mesajım var mı" demez; hook tur sınırında
  `hook.turn_end` çağırır, daemon bekleyen batch'i döndürür.
- **Pointer mesaj (K4):** `summary ≤ 400` hem MCP kenarında (S4 §3.4) hem DB CHECK'te;
  payload > 2KB reddedilir → `artifact_ref`.
- **Tur sınırı batching (K5):** bekleyenler tek `[DIVAN MESSAGES]` bloğu; ilk satır sabit
  yönerge ("bu kullanıcı mesajı değil; yalnız `send_message` ile cevap ver").
- **Kapsamlı abonelik (K6):** broadcast yok; `subscribe(event_kind, filter)` + `broadcast`
  capability'si olmadan topic mesajı reddedilir (`policy_denied` trace).

## Policy chokepoint (F2.3) ve canlı çakışma (F2.6)

- **Policy chokepoint:** her MCP tool handler `policy::check(db, agent, action)`'tan geçer
  (impl plan §F2.3). Faz 2'de fiilen uygulanan capability'ler: `broadcast` (bus'ta),
  `delegate` (delegate_task), `read` (subscribe). Diğer eylemler chokepoint'ten geçer ama
  Faz 2'de açıktır; tam capability matrisi (spec §5.1) Faz 3'te (F3.1) bu noktaya takılır.
  Red → `policy_denied` trace.
- **Canlı çakışma (F2.6):** scheduler artık her canlı `FileEdit` olayında
  `MessageBus::record_touch` çağırır → `file_touches` yazılır; 30 sn içinde başka ajan aynı
  path'e dokunursa pointer alert ilgili ajanlara gider. (Önceki sürümde yalnız test çağırıyordu.)

## İki teslim yolu

- **MCP (ajan → hub):** `divan-mcp` stdio sunucusu, 8 araç (`send_message`, `delegate_task`,
  `claim_task`, `complete_task`, `publish_artifact`, `get_artifact`, `subscribe`,
  `list_agents`). Her araç daemon soketine forward eder. `divan mcp print-config --agent <id>`
  kayıt config'i üretir (`DIVAN_AGENT_ID` ile kimlik).
- **Hook (hub → ajan, push):** `divan install-hooks --tool claude`. Claude'un
  `UserPromptSubmit` hook'u enjeksiyon kanalı (S3), `Stop` hook'u activity/idle sinyali.
  Kurulum mevcut hook'larla **birleşir**, **yedek alır**, **asla ezmez**; daemon kapalıyken
  script sessizce `exit 0` (ajan bozulmaz). Codex: hook yok → **MCP-only** (Faz 0 S2/S5).

## F2.7 demo senaryosu

Otomatik kapsama (her `cargo test`'te koşar):
- `crates/divan-daemon/tests/messaging_rpc.rs` — soket üstünden "toy MCP client":
  send → `hook.turn_end` peek → `hook.confirm` → kuyruk boşalır; broadcast capability/abonelik
  geçidi; publish/get artifact + list_agents.
- `divan-daemon::bus` birim testleri — fan-out, receipts, conflict alert.
- `divan-hooks` — gerçek `/bin/sh` ile no-op-when-down + merge/backup/idempotent/uninstall.

Canlı manuel demo (gerçek claude + codex, opsiyonel — gerçek `~/.claude` config'i değiştirir):
```bash
divan up                                   # hedef repo dizininden
divan install-hooks --tool claude          # ~/.claude'a hook'ları merge eder (yedekli)
divan mcp print-config --agent codex-1 > /tmp/divan-mcp.json
# codex'i divan-mcp ile başlat; codex subscribe('question') + send_message ile cevap verir
# claude implement sırasında send_message('question') ile codex'e sorar;
# cevabı tur sınırında hook injection ile [DIVAN MESSAGES] bloğunda alır.
divan log                                  # tüm trafik: msg_sent / msg_delivered / artifact_published
```

CLI komutları (Faz 2): `divan agents`, `divan messages [--agent <id>] [--task <id>]`,
`divan install-hooks`, `divan uninstall-hooks`, `divan mcp print-config`.

## Bilinen sınırlar (Faz 2 — bilinçli)

- **Canlı uçtan-uca demo otomatik koşulmadı** (gerçek `~/.claude` hook kurulumu + token
  harcar). MCP ve hook yolları izole testlerle doğrulandı; MCP yüzü gerçek claude ile
  canlı doğrulandı; tam hook-injection canlı demosu kullanıcı tetikler.
- **Policy Engine minimal.** Chokepoint var; fiilen `broadcast`/`delegate`/`read` geçitleri.
  Tam capability matrisi + PreToolUse path kontrolü **Faz 3**.
- **Cost Router yok** (Faz 3). delegate/claim deterministik "en eski açık task" seçer.
- **Batch header tek trace/task** taşır (ilk mesajınki); çok-trace batch'te her madde kendi
  pointer'ını listeler.
- **MCP `get_artifact` yalnız UTF-8** döndürür; ikili içerik Faz 2+ .
- **`message_deliveries` tablosu yok** (P0.2 Seçenek B); receipt `messages.delivered_at`'te.

## Faz 3'e devir

Policy Engine (capability matrisi, PreToolUse path, `policy_denied`), Cost Router (YAML,
`different_vendor_than`, `multi_turn` exclude), CopilotAdapter (`--allow-tool` ↔ policy),
AgyAdapter (degraded one-shot).
