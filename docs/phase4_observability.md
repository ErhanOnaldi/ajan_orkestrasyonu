# Divan — Faz 4 Durumu: Gözlemlenebilirlik + TUI

Kaynak: `general_plan_and_architecture.md` §5.3 · `implementation_plan.md` §F4.
Tarih: 2026-06-13.

## Tamamlanan iş paketleri

| Paket | Konu | Durum |
| --- | --- | --- |
| F4.1 | Trace query API (timeline, span grouping, metrics aggregate) | ✅ |
| F4.2 | Ratatui ana ekran (4 panel, keyboard nav) | ✅ (`--once` headless doğrulandı; canlı kullanıcı-yönlü) |
| F4.3 | Token/maliyet metrikleri (CLI) | ✅ |
| F4.4 | HTML export sınırı (v1.5 roadmap) | ✅ not |

**Test durumu:** `cargo test --workspace` → **164 test** geçiyor. `cargo fmt` + `cargo clippy
--workspace --all-targets` temiz.

## F4.1 Trace query + F4.3 metrikleri

`divan-trace`:
- `Timeline::render_text` — sağlam metin timeline (`divan trace <id>`); eksik/bilinmeyen
  event alanlarına toleranslı (F4.1 kabul: "eksik event timeline'ı kırmaz").
- `Timeline::spans` — span_id'ye göre gruplama (TUI şeritleri için).
- `compute_metrics` — mesaj sayısı, ortalama/max summary uzunluğu (K4 ≤400 disiplini),
  injection sayısı (`msg_delivered`), tur sayısı, görev başına cost_class dağılımı.
  **Yalnız proxy ölçüler; "yüzde tasarruf" iddiası YOK** (spec §9).

Daemon `trace` RPC trace-id veya task-id çözer; `divan trace <id>` timeline + metrikleri
yazdırır. Canlı doğrulandı: write-review trace'i tam render edildi (transitions, spawn,
file_edit, turn_end, artifact_published; turns=2, cost_class_dist c4=1 c5=1).

## F4.2 TUI (`divan tui` / `divan-tui`)

ratatui + crossterm, 4 panel: **Agents** (id/tool/status/cost), **Tasks** (gezinilebilir
liste), **Messages** (akış), **Trace/detail** (seçili task'ın trace'i). ~1 sn poll; daemon
soketinden besler (mevcut RPC'leri kullanır, mantık daemon'da). Klavye: ↑/↓ (j/k) task
seçimi, Enter trace yükle, Tab panel odağı, q/Esc çıkış (panic'te bile terminal restore).
Daemon kapalıysa "daemon not running" gösterir, panik yok.

**Test edilebilirlik:** veri katmanı (`snapshot`) + render (`ui`) saf/TTY'siz test edilir;
`--once` headless modu tek frame'i `TestBackend` ile metne render eder (smoke yolu). 12 test.
**Canlı interaktif 3-ajan görünümü gerçek terminal gerektirir (kullanıcı-yönlü).** `--once`
ile 4 panel canlı daemon'a karşı doğrulandı.

## F4.4 HTML export sınırı

Statik HTML export (`divan trace export --format html`) **v1.5 kapsamındadır**, Faz 4 kabul
kriteri DEĞİLDİR (P0.1). v1 için `divan trace <id>` metin/TUI timeline yeterlidir. README
roadmap'ine v1.5 maddesi olarak girer (Faz 5 F5.3).

## Bilinen sınırlar (Faz 4)

- TUI canlı interaktif demo kullanıcı-yönlü (TTY gerekir); CI'da `--once` + birim testler.
- Dar terminalde panel içerikleri kırpılır (TUI doğası); geniş terminal önerilir.
- HTML export v1.5 (yukarıda).

## Faz 5'e devir

README (İngilizce, K1–K10 özeti, diyagram, GIF, install), ADR'ler, CI (GitHub Actions:
fmt/clippy/test, macOS/Linux), paketleme (`cargo install` + shell installer), v1.5 HTML export.
