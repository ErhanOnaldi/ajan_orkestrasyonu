# Divan — Faz 0 Doğrulama Spike'ları

Bu dizin **throwaway** doğrulama kodunu ve raporlarını barındırır.
Spec: `general_plan_and_architecture.md` §7 (Faz 0) · `implementation_plan.md` §9 F0.

> **Kural:** Spike kodu üretim crate'lerine kopyalanmaz. Faz 0 çıktısı =
> 5 spike raporu + dil kararı (ADR 0001) + doğrulanmış §3.3 uyumluluk matrisi.

## Format

Her spike `TEMPLATE.md` yapısını izler: amaç · komutlar · gözlemler ·
ham çıktı konumu · karar · üretim etkisi. Ham çıktılar `raw/` altında, secret'sız.

## Spike durumu

| Spike | Konu | Hedef araç | Durum |
| --- | --- | --- | --- |
| S0 | Araç uygunluk taraması (`tool_check.sh`) | hepsi | ✅ tamam (`raw/s0_tool_check.txt`) |
| S1 | `claude -p --output-format stream-json` parse + event normalize | claude ✅ | ✅ tamam (`s1_claude_stream_json.md`) |
| S2 | `codex exec --json` event modeli + ortak `NormalizedEvent` | codex ✅ | ✅ tamam (`s2_codex_json.md`) |
| S3 | Claude hook enjeksiyonu + Stop sinyali + no-op | claude ✅ | ✅ tamam (`s3_hook_injection.md`) |
| S4 | `rmcp` 2-tool oyuncak MCP server + claude roundtrip | claude ✅ + rust ✅ | ✅ tamam (`s4_rmcp_toy_server.md`) |
| S5 | `copilot -p` / `agy -p` davranışı, `--allow-tool`, issue #7 | copilot ✅ / agy ✅ | ✅ tamam, audit sonrası düzeltildi (`s5_copilot_agy.md`) |

## Faz 0 kabul kapısı — KARŞILANDI (2026-06-11)

Spec §7 Faz 0 kabul kriteri: *"5 kısa spike raporu (md) + dil kararının kilitlenmesi +
3.3 matrisinin doğrulanmış sürümü."*

- ✅ **5 spike raporu** (S1–S5) aynı `TEMPLATE.md` formatında, sanitize ham çıktılar `raw/` altında.
- ✅ **Dil kararı kilitlendi:** `docs/adr/0001-language-and-runtime.md` — Rust devam, B planı tetiklenmedi, P0.3 kilidi kapatıldı.
- ✅ **§3.3 matrisi doğrulandı:** güncellenmiş sürüm `s5_copilot_agy.md` §5'te (sürüm numaraları + spike referanslarıyla). Agy için desteklenen stdout/stderr ID yüzeyi yoktur; private `~/.gemini/antigravity-cli` cache/log ID'si v1 adaptör sözleşmesi sayılmaz.

**Faz 0 çıktısı = bu dizin (throwaway) + ADR 0001. Üretim hub kodu (`crates/`) Faz 1'de başlar.**
Faz 1'e devreden notlar: `rusqlite` bloklama önlemi, `rmcp` 1.7.0 sürüm sabitleme + API
notları, adaptörlerde iki normalize stratejisi (JSONL parser vs text+diff), Agy için
private cache/log scraping'e dayanmayan degraded one-shot tasarım.

## Araç envanteri (2026-06-11 taraması — `raw/s0_tool_check.txt`)

Tüm Faz 0 hedef araçları mevcut: claude 2.1.173, codex-cli 0.139.0,
copilot 1.0.48, agy 1.0.7, opencode 1.14.50; rust 1.92.0.
`codex` ve `agy` `~/.local/bin` altında (sembolik link / standalone binary).
