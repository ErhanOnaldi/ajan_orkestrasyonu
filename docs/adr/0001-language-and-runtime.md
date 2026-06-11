# ADR 0001 — Dil ve Runtime Kararı

| Alan | Değer |
| --- | --- |
| Durum | **Kabul edildi ve KİLİTLENDİ** (Faz 0 kabul kapısı) |
| Tarih | 2026-06-11 |
| Karar verici | Erhan Önaldı (proje sahibi) |
| Bağlam | Faz 0 doğrulama spike'ları (S1–S5) tamamlandı |
| İlgili spec | `general_plan_and_architecture.md` §6, §11 (P0.3) · `implementation_plan.md` §2 P0.3, F0.7 |

---

## Bağlam

Spec §6 ana dili **Rust** olarak belirler, **C#/.NET 8'i B planı** olarak tutar. P0.3
kuralı: B planına geçiş **yalnızca Faz 0 kabul kapısında** tetiklenebilir; Faz 1
başladıktan sonra dil değişikliği yapılamaz. Bu ADR o kapıdaki kararı kayda geçirir.

Faz 0'da Rust'ın ilerleme hızı, crate olgunluğu ve araç uyumu ampirik olarak test edildi:

- **S1 (claude stream-json):** JSONL akışı `serde_json` ile satır-satır parse edildi;
  `NormalizedEvent` eşlemesi sorunsuz. ✅
- **S2 (codex exec --json):** thread/turn/item modeli aynı JSONL okuyucuyla çözüldü;
  ortak `NormalizedEvent` modeli çıkarıldı. ✅
- **S3 (hook injection):** Teslim yolu POSIX `sh` hook'ları + (üretimde) Unix socket
  RPC ile çözülüyor — dil-bağımsız; Rust tarafı yalnız ince RPC. ✅
- **S4 (rmcp toy server):** **Resmi Rust MCP SDK `rmcp` 1.7.0 stabil**; `#[tool_router]` /
  `#[tool_handler]` makro yolu çalıştı, gerçek `claude` CLI ile tool-call roundtrip
  doğrulandı (send_message + publish_artifact, blake3 ref bağımsız teyitli). ✅
- **S5 (copilot/agy):** Adaptör kısıtları araç kaynaklı (düz metin, issue #7); dil seçimiyle
  ilgisiz. ✅

Toolchain mevcut: `cargo/rustc 1.92.0`. Spec §6'daki crate seti (`tokio`, `serde`,
`rusqlite`, `clap`, `ratatui`, `rmcp`, `notify`, `blake3`, `tracing`) için engelleyici
bulunmadı; `rmcp` ve `blake3` Faz 0'da fiilen derlendi.

## Karar

**Rust ile devam edilir.** Tek Cargo workspace; hub + adaptörler + CLI/TUI Rust.
Hook script'leri her durumda POSIX shell/Python kalır (§6, P0.4).

**B planı (C#/.NET 8) TETİKLENMEDİ.** Faz 0 hiçbir Rust-kaynaklı engelle karşılaşmadı;
en riskli entegrasyon (rmcp MCP server) ilk denemede çalıştı.

## Kilit kuralı (P0.3 — bağlayıcı)

> B planına (C#/.NET 8) geçiş **yalnızca Faz 0 kabul kapısında** tetiklenebilir.
> Bu kapı bu ADR ile **kapanmıştır**. **Faz 1 başladıktan sonra dil değişikliği
> YAPILAMAZ.** Bu kuralı değiştirmek insan onayı + bu ADR'nin "Superseded" işaretlenmesini
> gerektirir.

## Sonuçlar

- **Olumlu:** Tek-binary dağıtım, güçlü tipler/exhaustive enum (AGENTS.md kodlama
  kuralları), daemon/süreç yönetiminde olgun ekosistem, doğrulanmış MCP SDK.
- **Maliyet:** Rust öğrenme/derleme süresi; `rusqlite` blocking → daemon event loop'unda
  SQLite actor / `spawn_blocking` sınırı zorunlu (implementation_plan §3, risk tablosu).
- **Geri dönülmezlik:** Faz 1 başladığında bu karar dondurulur; sonraki itirazlar
  v2 kapsamına yazılır, koda dönüşmez.

## İzlenecek (Faz 1'e devreden)

- `rusqlite` async bloklama önlemi (SQLite actor / `spawn_blocking`) — F1.2/F1.3.
- `rmcp` 1.7.0 sürüm sabitleme; API notları (S4 raporu §6): `Parameters` →
  `handler::server::wrapper`, `ServerInfo`/`Implementation` `#[non_exhaustive]` (builder
  zinciri), `get_info` senkron.
