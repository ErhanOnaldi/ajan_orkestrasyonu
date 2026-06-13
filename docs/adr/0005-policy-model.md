# ADR 0005 — Yetenek Bazlı İzin Modeli (Policy Engine)

| Alan | Değer |
| --- | --- |
| Durum | **Kabul edildi** (K8 bağlayıcı kararı) |
| Tarih | 2026-06-13 |
| Karar verici | Erhan Önaldı (proje sahibi) |
| Bağlam | Faz 3 Policy Engine + MCP chokepoint üretim kodunda |
| İlgili spec | `general_plan_and_architecture.md` §2 (K8), §5.1 · `implementation_plan.md` Faz 3 |
| İlgili referans | hcom / swarm-protocol "total trust" modeli — *karşıt örnek* |

---

## Bağlam

Ekosistemdeki çok-ajan koordinasyon katmanları (hcom, swarm-protocol) genelde
"total trust" modelindedir: bir ajan spawn edebilir, kill edebilir, her yere
yazabilir. Bu, Divan'ın farklılaşma noktasının tam tersidir. Güvenli bir hub,
her eylemi yetkiye karşı denetlemelidir.

## Karar

**Her ajan kaydında bir capability seti bulunur (`read`, `write`, `spawn`,
`kill`, `delegate`, `broadcast`); varsayılan en dar yetkidir ve çalışma anında
genişletilemez. Her eylem Policy Engine'den geçer; ihlal trace'e yazılır ve
reddedilir.**

- **MCP chokepoint:** Ajanların hub'a tüm yetkili eylemleri MCP tool'larından
  geçer; her handler `policy::check` üzerinden denetlenir (bypass yok).
- **Execution matrix:** `check_write` (write capability + worktree içi yol),
  `check_kill` (kill capability + sahiplik), `check_spawn` (spawn capability +
  maliyet sınırı).
- **Path canonicalization:** lexical normalizasyon `..` kaçışını engeller;
  macOS symlink uyumsuzluğu için her iki taraf da lexical normalize edilir
  (`fs::canonicalize` değil).
- **Red kaydı:** `PolicyDenied { agent, action, reason }` → `policy_denied`
  trace olayı (K10). Sınıflandıramayan durumda reddet (fail-safe).

## Sonuçlar

- **Olumlu:** "Total trust" karşıtı bir güvenlik modeli; her reddin denetim izi
  var; write boundary worktree izolasyonuyla (K7) birleşir. Capability'ler
  kayıt zamanında sabit — runtime privilege escalation imkânsız.
- **Maliyet:** Her eylemde ek denetim katmanı; capability setini doğru
  modellemek kayıt tarafında dikkat gerektirir.
- **Sınır:** Capability'lerin runtime'da genişletilememesi bağlayıcıdır; daha
  geniş yetki yeni bir ajan kaydı/insan onayı gerektirir, kod yolu değil.
