# ADR 0003 — Pointer Mesajlar: İçerik Değil Referans Taşı

| Alan | Değer |
| --- | --- |
| Durum | **Kabul edildi** (K4 bağlayıcı kararı) |
| Tarih | 2026-06-13 |
| Karar verici | Erhan Önaldı (proje sahibi) |
| Bağlam | Faz 2 mesaj omurgası + Artifact Store üretim kodunda |
| İlgili spec | `general_plan_and_architecture.md` §2 (K4), §4.4, §4.5 · `implementation_plan.md` Faz 2 |

---

## Bağlam

Ajanlar arası mesajda dosya içeriği/diff/uzun çıktı taşımak iki şeyi bozar:
context hijyenini ve maliyeti. Dosya sisteminde zaten duran (veya artifact
deposuna yazılabilen) bilgiyi mesaja gömmek, alıcı ajanın context'inde her turda
yeniden ücretlenir. Mesaj bir *bildirim*tir, bir *veri kanalı* değil.

## Karar

**Mesajlar pointer + yapılandırılmış özet taşır; ağır içerik Artifact deposuna
gider.**

- Mesaj gövdesi: artifact referansı (blake3 ile içerik-adresli) + **≤400
  karakter** yapılandırılmış özet. Büyük içerik mesajda **asla** taşınmaz.
- Özet uzunluğu kod tarafında doğrulanır (sınır aşımı reddedilir).
- Artifact Store içerik-adresli ve dedupe'lidir (aynı içerik → aynı ref).
- DB şemasında mesaj satırı içerik kolonu tutmaz; yalnız `spec_ref` + özet.

## Sonuçlar

- **Olumlu:** Context temiz kalır; prompt-cache prefix'i korunur (K5 ile);
  aynı artifact'a birden çok mesaj referans verebilir (dedupe). Özet ≤400 sınırı
  mesajı gerçekten "pointer" tutmaya zorlar.
- **Maliyet:** Alıcı ajanın içeriği görmek için ek bir tool çağrısı
  (`get_artifact`) yapması gerekir — bu bilinçli takastır: ihtiyaç anında çek,
  her turda taşıma.
- **Sınır:** 400 karakter özet disiplini bağlayıcıdır; özet üretimi adaptör/iş
  ajanı sorumluluğundadır, hub yalnız doğrular ve reddeder.
