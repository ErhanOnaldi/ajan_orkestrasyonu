# Spike S3 — Claude hook enjeksiyonu + Stop sinyali

> Faz 0 doğrulama spike'ı. **Throwaway; üretim crate'lerine kopyalanmaz.**
> Spec: `general_plan_and_architecture.md` §3.6 (Yol A), K1/K3/K5 · `implementation_plan.md` F0.4 / F2.1–F2.2

| Alan | Değer |
| --- | --- |
| Spike | S3 |
| Hedef araç | claude |
| Araç sürümü | `2.1.173 (Claude Code)` |
| Tarih | 2026-06-11 |
| Durum | ✅ tamamlandı |

---

## 1. Amaç

K1/K3/K5'in temelini doğrulamak: Claude Code hook'larıyla (a) çalışan oturuma tur
sınırında bir mesaj bloğu **enjekte** edilebiliyor mu (push, poll değil), (b) Stop
hook'undan hub'a **aktivite/turn-end sinyali** gidebiliyor mu, (c) Divan kapalıyken
hook'lar **no-op** kalıyor mu (kullanıcının CLI'ını bozmadan)?

## 2. Komutlar

Toy hub: `$DIVAN_HUB/pending.txt` (teslim kuyruğu) + `$DIVAN_HUB/activity.log` (sinyal).
İki POSIX `sh` hook'u `.claude/settings.json`'a bağlandı:

```json
{ "hooks": {
  "UserPromptSubmit": [ { "hooks": [ { "type":"command", "command":".../userprompt_inject.sh" } ] } ],
  "Stop":            [ { "hooks": [ { "type":"command", "command":".../stop_signal.sh" } ] } ]
}}
```

```bash
# ON: hub dolu -> enjeksiyon beklenir, token echo edilmeli
DIVAN_HUB=$HUB claude -p "...reply with ONLY the token after 'token='..." \
  --output-format stream-json --verbose --max-turns 1 < /dev/null
# OFF: DIVAN_HUB unset -> hook no-op, token yok
claude -p "...same prompt..." --output-format stream-json --verbose --max-turns 1 < /dev/null
```

## 3. Gözlemler

| Senaryo | Beklenen | Gözlenen |
| --- | --- | --- |
| Divan ON | model token'ı görür | `result = ZX9Q` ✅ (enjekte edilen bloktan) |
| Divan ON | Stop sinyali | `activity.log` ← `…Z session_stop turn_end` ✅ |
| Divan OFF | enjeksiyon yok | `result = NONE` ✅ |
| Divan OFF | sinyal yok | `activity.log` = 0 byte ✅ |

Doğrulanan gerçekler:
- **`UserPromptSubmit` hook'unun stdout'u (exit 0) doğrudan context'e eklenir.** Tur
  sınırı enjeksiyonunun (§3.6 Yol A, K5) mekanizması budur. Enjekte edilen
  `[DIVAN MESSAGES]` bloğu — sabit davranış yönergesi satırı dâhil — context'e ulaştı;
  model token'ı okudu.
- **`Stop` hook'u oturum bitiminde tetiklenir** → aktivite kaydı + idle/turn-end sinyali
  (daemon'ın boştaki oturumu bilmesi için). Headless `-p` modunda da çalışır.
- **No-op güvenli:** Hook'lar `DIVAN_HUB` yokken veya kuyruk boşken çıktı üretmeden
  `exit 0` döner; CLI davranışı değişmez. Bu, "Divan kullanılmıyorsa hook'lar no-op
  olmalıdır" (§3.6) kuralının kanıtıdır.
- Proje-seviyesi `.claude/settings.json` hook'ları headless modda ek onay olmadan
  çalıştı.

## 4. Ham çıktı konumu

`docs/spikes/raw/s3_hook_injection.txt` (ON + OFF çıktıları, secret yok).

## 5. Karar

§3.6 Yol A (hook teslimatı) **doğrulandı**. Üretim hook mimarisi:
- **Enjeksiyon kanalı = `UserPromptSubmit` hook'u** (stdout → context, tur sınırında).
- **Aktivite/idle sinyali = `Stop` hook'u** (+ Faz 2'de idle-wake için ek yol).
- Hook'lar **ince**: tüm iş mantığı (kuyruk seçimi, batch oluşturma) hub RPC'sine
  taşınır; hook yalnız `DIVAN_HUB` soketinden/dizininden okur (implementation_plan
  riski "Hook API değişimi → minimal hook logic"). Bu spike'ta dosya kullanıldı;
  üretimde Unix socket RPC (F2.2 `turn.end`/`activity.report`) olacak.
- Enjekte edilen blok **§5.3 batch formatına birebir uyar** (instruction satırı +
  numaralı pointer'lar + opsiyonel `artifact=`).

## 6. Üretim etkisi (Faz 2 hook paketi)

- `divan install-hooks`: `UserPromptSubmit` + `Stop` (+ Codex eşleniği S5/F2.1) girdilerini
  kullanıcının mevcut `settings.json`'ına **merge** eder, ezmez, yedek alır (F2.1).
- Hook script'i: `DIVAN_SOCKET` set değilse / daemon yoksa sessiz `exit 0` (no-op kanıtı).
- Enjeksiyon bloğu sabit instruction satırını içerir (§3.6, §5.3) — ajanın serbest metinle
  cevap verip token sızdırmasını önler (K3/K4).
- Batch hub tarafında üretilir; hook yalnız hazır bloğu stdout'a basar (pointer-only, K4).
- `Stop` sinyali daemon'da `SessionIdle` normalize olayına çevrilir (S1/S2'de akıştan
  gelmeyen tek olay — bu hook onu tamamlar).
