# ADR 0002 — Deterministik Hub: Koordinasyonda Sıfır LLM Token'ı

| Alan | Değer |
| --- | --- |
| Durum | **Kabul edildi** (K2 bağlayıcı kararı) |
| Tarih | 2026-06-13 |
| Karar verici | Erhan Önaldı (proje sahibi) |
| Bağlam | Faz 1–4 implementasyonu; scheduler + bus + router üretim kodunda |
| İlgili spec | `general_plan_and_architecture.md` §2 (K2), §4.2, §5 · `implementation_plan.md` Faz 1–3 |
| İlgili referans | bernstein (test-doğrulamalı deterministik orkestrasyon) |

---

## Bağlam

Çok-ajanlı kurulumlarda koordinasyon genelde *model üzerinden* yapılır: ajan
"mesajım var mı" diye sorar, kuyruk/kilit/bağımlılık kararları prompt'a yazılır.
Token maliyeti bileşiktir — context'e giren her şey her turda yeniden işlenir —
bu yüzden saf defter-tutma işi faturanın en pahalı kısmı olabilir.

Kuyruklama, yönlendirme, kilitleme ve bağımlılık çözümü ise tamamen deterministik
işlemlerdir; LLM gerektirmezler.

## Karar

**Tüm koordinasyon hub'ın deterministik Rust kodunda yapılır. LLM yalnızca LLM
gerektiren işte (kod yazma, review, plan) kullanılır.**

Bu kararın kodda karşılığı:

- **Task Scheduler** — açık durum makinesi (`open → claimed → working → review →
  done`, `+failed/cancelled`); geçişler yalnızca `TaskStore::transition` ile
  atomik (durum + trace tek yazımda). Bağımlılık grafı (DAG): bir task `done`
  olunca blokesi kalkan task'lar `open`'a çekilir. Hiçbir adımda model çağrısı
  yok.
- **Message Bus** — abonelik eşleştirme, fan-out, batching; hepsi kod.
- **Cost Router** — YAML kural motoru; skor + deterministik tiebreak (skor desc,
  multi_turn, cost_class asc, id asc). Aynı girdi → aynı çıktı, model yok.
- **Error taksonomisi** — `retryable | fatal` sınıfı adaptörden gelir; retry
  kararı bu sınıfa göre deterministik verilir (varsayılan 2 deneme / `failed`).

## Sonuçlar

- **Olumlu:** Koordinasyon token'ı sıfır; davranış tekrarlanabilir ve test
  edilebilir (integration testler fake adapter ile akışı doğrular); "neden bu
  karar" sorusu trace'ten yanıtlanır (K10 ile birlikte).
- **Maliyet:** Yönlendirme/önceliklendirme "akıllı" değil kural-tabanlı başlar;
  zekâ v1.5'te trace telemetrisiyle skor düzeltmesine bırakılır (K9).
- **Sınır:** Model yalnızca iş ajanlarının içinde; hub asla prompt üretip karar
  için model çağırmaz. Bu sınırı gevşetmek bu ADR'nin gözden geçirilmesini
  gerektirir.
