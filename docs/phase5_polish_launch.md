# Divan — Faz 5 Durumu: Cila ve Lansman

Kaynak: `implementation_plan.md` §F5.
Tarih: 2026-06-13.

Amaç: Dış kullanıcının ~10 dakikada kurup write-review akışını çalıştırabileceği
paket + dokümantasyon.

## İş paketleri

| Paket | Konu | Durum |
| --- | --- | --- |
| F5.1 | CI (GitHub Actions: fmt/clippy/test, build macOS+Linux, MSRV) | ✅ |
| F5.2 | Paketleme (`cargo install` + shell installer + uninstall) | ✅ |
| F5.3 | Dokümantasyon (İngilizce README, ADR'ler, CONTRIBUTING) | ✅ (demo GIF kaydı kullanıcı-yönlü) |
| F5.4 | Lansman hazırlığı (Show HN / reddit / awesome-list / LinkedIn taslakları) | ✅ taslak (yayınlama kullanıcı-yönlü) |

## F5.1 CI

`.github/workflows/ci.yml`:
- **lint** (ubuntu): `cargo fmt --all -- --check` + `cargo clippy --workspace
  --all-targets -- -D warnings`.
- **test** (matrix: ubuntu + macos): `cargo build --workspace --all-targets
  --locked` + `cargo test --workspace --locked`.
- **msrv** (ubuntu, Rust 1.88): MSRV derleme guard'ı.
- `-D warnings` yalnız clippy'ye uygulanır (global RUSTFLAGS DEĞİL) — aksi halde
  bağımlılık uyarıları build/test/MSRV'yi düşürürdü.
- Gerçek-CLI testleri env-gated (`DIVAN_TEST_*`); CI bu değişkenleri **set
  etmez**, dolayısıyla harici CLI yoksa CI fail olmaz (F5.1 kabul).

**MSRV düzeltmesi:** `Cargo.toml` `rust-version` 1.80 → **1.88** olarak düzeltildi.
Bağımlılık ağacı (darling, rmcp/schemars üzerinden) 1.88 ister; 1.80 yanlış bir
iddiaydı ve hem `cargo install` kullanıcısını hem MSRV CI job'ını düşürürdü.

## F5.2 Paketleme

- `install.sh` (POSIX sh, macOS+Linux): dört binary'yi (`divan`, `divan-daemon`,
  `divan-mcp`, `divan-tui`) `cargo install --path … --locked` ile aynı
  `~/.cargo/bin` dizinine kurar — CLI'nin sibling-lookup'ı tam olarak bunu bekler.
  `--with-hooks` ile hook kurulumunu da yapar. PATH'te değilse kullanıcıya satırı
  söyler (shell config'i ezmez).
- `Cargo.toml` `repository` URL'i gerçek remote'a (`ajan_orkestrasyonu`)
  düzeltildi (eski `…/divan` yanlıştı).
- `LICENSE` (MIT) eklendi (manifest `license = "MIT"` diyordu ama dosya yoktu).
- Uninstall: `divan uninstall-hooks` + `cargo uninstall …` (README'de belgeli).
- Installer hook'ları **merge eder, ezmez** (divan-hooks: backup + merge).

## F5.3 Dokümantasyon

- **README.md** (İngilizce): problem, K1–K10 tablosu, ASCII mimari diyagram,
  install, write-review quickstart, CLI referansı, **Limitations** (yüzde tasarruf
  iddiası YOK), roadmap (v1.5 HTML export), license. Demo GIF için işaretli
  TODO (kayıt kullanıcı-yönlü).
- **ADR'ler:** 0001 (dil/runtime, mevcut) + yeni 0002 (deterministik hub/K2),
  0003 (pointer mesajlar/K4), 0004 (worktree izolasyonu/K7), 0005 (policy
  modeli/K8).
- **CONTRIBUTING.md:** spec-first kurallar, crate haritası, test/CI komutları,
  env-gated gerçek-CLI testleri, definition of done.

## F5.4 Lansman hazırlığı

`docs/launch.md`: Show HN, r/ClaudeCode, awesome-agent-orchestrators PR girdisi,
LinkedIn başlık serisi taslakları. Tüm metinler README Limitations ile hizalı
(yüzde tasarruf iddiası yok). `<REPO_URL>`/`<DEMO_LINK>` yer tutucuları + go-live
checklist'i; **yayınlama kullanıcı-yönlü**.

## Bilinen sınırlar / kullanıcı-yönlü kalemler

- **Demo GIF** kaydı gerçek terminal oturumu gerektirir (kullanıcı-yönlü);
  README'de işaretli TODO.
- **Sosyal yayınlama** (Show HN/reddit/LinkedIn) ve **awesome-list PR'ı** proje
  sahibi tarafından yapılır; taslaklar hazır.
- **Temiz makine kurulum doğrulaması** (≤10 dk) gerçek temiz makine ister;
  installer + docs adımları buna göre yazıldı.
- HTML trace export **v1.5** (Faz 5 kapsamı dışı, README roadmap'inde).
