# ADR 0004 — Görev Başına Worktree İzolasyonu

| Alan | Değer |
| --- | --- |
| Durum | **Kabul edildi** (K7 bağlayıcı kararı) |
| Tarih | 2026-06-13 |
| Karar verici | Erhan Önaldı (proje sahibi) |
| Bağlam | Faz 1 worktree manager + Faz 3 write boundary üretim kodunda |
| İlgili spec | `general_plan_and_architecture.md` §2 (K7), §3.4, §5.1 · `implementation_plan.md` Faz 1, Faz 3 |
| İlgili referans | crystal, vibe-kanban (worktree izolasyon deseni) |

---

## Bağlam

Paralel yazan ajanlar aynı çalışma ağacında çakışır: yarış koşulları, yarım
commit'ler, birbirini ezen düzenlemeler. Dosya kilidi yerine git'in kendi
izolasyon primitifi — worktree — kökten çözüm sunar ve ekosistemde kanıtlanmıştır.

## Karar

**Yazma yetkili her görev kendi git worktree'sinde koşar; merge insan onayıyla,
asla otomatik değil.**

- Her write task için izole worktree + branch oluşturulur.
- Diff, worktree'nin commit edilmemiş düzenlemelerini de kapsar (review gerçek
  değişikliği görür).
- `divan merge <task-id>` açıktır ve yalnız insan tetikler; hub asla kendiliğinden
  merge etmez.
- `divan cleanup <task-id>` worktree'yi kaldırır ama **artifact'lara dokunmaz**.
- Write boundary (K8 ile): yazma, görevin worktree'siyle sınırlanır; lexical path
  normalizasyonu `..` kaçışını engeller; Copilot adaptöründe bu sınır araç
  tarafında `write(<worktree>/**)` desenine çevrilir.

## Sonuçlar

- **Olumlu:** Dosya çakışması yapısal olarak imkânsız; her görevin değişikliği
  izole, gözden geçirilebilir, geri alınabilir. Merge bir insan kapısıdır.
- **Maliyet:** Worktree oluşturma/temizleme yaşam döngüsü yönetimi gerekir
  (oluştur → diff → merge/cleanup); disk kullanımı görev başına artar.
- **Sınır:** Merge'in her zaman manuel olması bağlayıcıdır — otomatik merge v1
  kapsamı dışıdır ve bu kararın değişmesini gerektirir.
