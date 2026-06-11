# Spike SX — <başlık>

> Faz 0 doğrulama spike'ı. **Bu kod/çıktı throwaway'dir; üretim crate'lerine kopyalanmaz.**
> Spec referansı: `general_plan_and_architecture.md` §<bölüm> · `implementation_plan.md` F0.<n>

| Alan | Değer |
| --- | --- |
| Spike | S<n> |
| Hedef araç | claude / codex / copilot / agy / opencode |
| Araç sürümü | `<komut --version çıktısı>` |
| Tarih | YYYY-MM-DD |
| Durum | ✅ tamamlandı / ⚠️ kısmi / ⛔ bloke (sebep) |

---

## 1. Amaç

Hangi riskli varsayım doğrulanıyor? Spec'in hangi kararı (K1–K10) veya hangi adaptör
gereksinimi bu spike'a bağlı?

## 2. Komutlar

Çalıştırılan tam komutlar (kopyalanabilir). Secret/token içermemeli.

```bash
# ...
```

## 3. Gözlemler

Ne görüldü? Çıktı formatı, event türleri, exit code'lar, timing, kısıtlar.
Tablolar ve örnek satırlar burada.

## 4. Ham çıktı konumu

Sanitize edilmiş ham örnek: `docs/spikes/raw/sN_*.txt`
(Kişisel token / secret taşımadığı doğrulandı: evet / hayır.)

## 5. Karar

Bu spike sonucunda alınan somut karar. Spec §3.3 matrisi veya adaptör tasarımı
üzerindeki etkisi.

## 6. Üretim etkisi

Faz 1+ üretim koduna taşınacak gereksinimler (kod değil, gereksinim listesi):
- NormalizedEvent mapping satırları
- Adaptör minimum spawn/observe/resume gereksinimleri
- Hata taksonomisi (`retryable` / `fatal`) eşlemeleri
