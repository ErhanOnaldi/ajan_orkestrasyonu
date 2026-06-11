# Divan — Çapraz Ajan Orkestrasyon Hub'ı: Mimari Spesifikasyonu ve Proje Planı

> Proje adı: **Divan** (ajanların toplanıp karar aldığı meclis).
> Sahip: Erhan Önaldı · Sürüm: v0.3 · Haziran 2026 · Değişiklikler için Bölüm 12'ye bakınız.

---

## 0. Bu Doküman Hakkında (ajan talimatları)

Bu doküman, Divan projesinde çalışan yapay zeka kodlama ajanları için yetkili (authoritative) spesifikasyondur. Aşağıdaki kurallara uyulmalıdır:

- Bölüm 2'deki K1–K10 kararları **bağlayıcı tasarım sözleşmesidir**. Bu kararlardan sapma gerektiren her durum implementasyona geçilmeden önce proje sahibine raporlanmalı ve onay beklenmelidir.
- Bölüm 7'deki faz sınırları kapsam sözleşmesidir. Aktif fazın kabul kriterlerinde yer almayan hiçbir özellik implemente edilmemelidir; yeni fikirler koda değil, v2 listesine not olarak eklenmelidir.
- "Kapsam DIŞINDA" listesindeki maddeler için kod yazılmamalı, soyutlama hazırlanmamalı, bağımlılık eklenmemelidir.
- Belirsizlikle karşılaşıldığında varsayım üretmek yerine Bölüm 10'daki referans projelerin ilgili implementasyonu incelenmeli; hâlâ belirsizse soru sorulmalıdır.
- Bu dokümanda güncelleme gerektiren bir tutarsızlık tespit edilirse kod değil, önce doküman düzeltilmelidir (spec-first).
- Bu spec'in eşlik dokümanı `implementation_plan.md`'dir (detaylı iş kırılımı). Çelişki durumunda bu spec geçerlidir; çelişki ayrıca Bölüm 12'ye kayıt düşülerek giderilmelidir.

---

## 1. Vizyon ve Kapsam

**Tek cümlelik tanım:** Claude Code, Codex, Copilot CLI, Antigravity CLI ve OpenCode gibi kodlama ajanlarını tek bir lokal hub üzerinden birbirine bağlayan; aralarında görev delegasyonu, mesajlaşma ve devir (handoff) sağlayan; izin, maliyet ve gözlemlenebilirlik katmanlarına sahip bir orkestrasyon aracı.

**Kapsam İÇİNDE (v1.0):**

- Lokal makinede çalışan CLI ajanlarının orkestrasyonu
- Ajanlar arası yapılandırılmış mesajlaşma (pointer tabanlı)
- Görev yaşam döngüsü yönetimi (A2A esinli Task modeli)
- İki teslim mekanizması: hook enjeksiyonu + MCP server
- Git worktree izolasyonu
- Yetenek bazlı izin modeli (capability scoping)
- Maliyet farkındalıklı görev yönlendirme (temel seviye)
- Ajan trafiği için trace/izleme

**Kapsam DIŞINDA (bilinçli karar — implemente edilmeyecek):**

- Araçların _iç_ subagent'larına müdahale (kapalı kutu — her aracın en üst ajanı hub için tek peer'dır)
- Tam A2A HTTP/JSON-RPC sunucusu (v1'de gereksiz ağ yükü; şema uyumlu tutulur, gateway v2)
- Çoklu makine / takım senaryosu (v2+ adayı)
- Hub'ın kendi LLM çağrıları (koordinasyon %100 deterministiktir)
- Web dashboard (v1'de TUI; trace HTML export v1.5)

---

## 2. Bağlayıcı Tasarım Kararları (K1–K10)

Tüm implementasyon bu tabloya uygun olmalıdır. Her karar, Bölüm 10'daki referans projelerin doğruladığı bir desene veya ekosistemde tespit edilmiş bir boşluğa dayanır.

| #   | Karar                                                                                                                                                                                            | Gerekçe                                                                                                                                                 |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| K1  | **Zarf A2A'dan, teslimat hook'tan.** Veri modeli A2A'nın AgentCard/Task/Message/Artifact şemasından uyarlanır; teslimat hook enjeksiyonu + idle wake ile yapılır (hcom deseni).                  | A2A'nın "ajan = HTTP server" varsayımı interaktif CLI'lara uymaz; hook'lar son kilometreyi çözer. Şema uyumu sayesinde ileride A2A gateway eklenebilir. |
| K2  | **Koordinasyon sıfır LLM token'ı.** Kuyruklama, yönlendirme, kilitleme, bağımlılık çözümü hub'ın deterministik kodunda yapılır. LLM yalnızca LLM gerektiren işte kullanılır (kod, review, plan). | Token maliyeti bileşiktir: context'e giren her şey her turda yeniden işlenir. (bernstein deseni)                                                        |
| K3  | **Push, asla poll değil.** Ajan "mesajım var mı" diye tool çağırmaz; hub mesajı hook ile tur sınırında enjekte eder veya boştaki ajanı uyandırır.                                                | Polling = her kontrolde tüm context'in yeniden ücretlendirilmesi.                                                                                       |
| K4  | **Mesajlar pointer taşır, içerik değil.** Ağır içerik Artifact deposuna yazılır; mesajda referans + ≤400 karakterlik yapılandırılmış özet gider.                                                 | Context hijyeni + maliyet. Dosya sisteminde duran bilgiyi context'ten taşımak israftır.                                                                 |
| K5  | **Tur sınırında batching.** Bekleyen mesajlar biriktirilir, tek kompakt blok olarak context'in SONUNA eklenir.                                                                                   | Kesinti azalır; prompt cache prefix'i bozulmaz.                                                                                                         |
| K6  | **Kapsamlı abonelik (scoped subscription).** Broadcast yoktur. İlgililik filtresi hub'da çalışır; ajan yalnızca abone olduğu olayları alır.                                                      | "Herkese her şey" modeli token ve context kirliliği üretir.                                                                                             |
| K7  | **Görev başına worktree izolasyonu.** Yazma yetkili her görev kendi git worktree'sinde koşar; merge insan onayıyla yapılır.                                                                      | Dosya çakışmasının kökten çözümü; ekosistemde kanıtlanmış desen (crystal, vibe-kanban).                                                                 |
| K8  | **Yetenek bazlı izin modeli.** Her ajan kaydında capability seti bulunur: `read`, `write`, `spawn`, `kill`, `delegate`, `broadcast`. Varsayılan: en dar yetki. Çalışma anında genişletilemez.    | Ekosistem boşluğu #1: hcom/swarm-protocol "total trust" modelindedir. Divan'ın farklılaşma noktası.                                                     |
| K9  | **Maliyet farkındalıklı yönlendirme.** Görev metadata'sı + ajan profili (model gücü, maliyet sınıfı, geçmiş performans) → router skoru.                                                          | Ekosistem boşluğu #2. YAML kural motoru olarak başlar, telemetriyle beslenir.                                                                           |
| K10 | **Her şey trace'lenir.** Her Task bir trace, her ajan etkileşimi bir span'dir. Mesajlar correlation ID taşır.                                                                                    | Ekosistem boşluğu #3: "bu karar neden alındı" sorusu izlenebilir olmalıdır.                                                                             |

---

## 3. Yüksek Seviye Mimari

```
┌─────────────────────────────────────────────────────────────┐
│  ARAYÜZLER                                                  │
│  divan CLI  ·  divan TUI (ratatui)  ·  [v1.5: trace HTML]   │
└──────────────┬──────────────────────────────────────────────┘
               │ IPC (unix socket / named pipe, JSON-RPC)
┌──────────────▼──────────────────────────────────────────────┐
│  HUB DAEMON (tek süreç, tokio)                              │
│  ┌────────────┐ ┌───────────┐ ┌──────────┐ ┌─────────────┐  │
│  │ Registry   │ │ Task      │ │ Message  │ │ Policy      │  │
│  │ AgentCards │ │ Scheduler │ │ Bus      │ │ Engine (K8) │  │
│  └────────────┘ └───────────┘ └──────────┘ └─────────────┘  │
│  ┌────────────┐ ┌───────────┐ ┌──────────┐ ┌─────────────┐  │
│  │ Cost       │ │ Artifact  │ │ Worktree │ │ Trace       │  │
│  │ Router(K9) │ │ Store     │ │ Manager  │ │ Collector   │  │
│  └────────────┘ └───────────┘ └──────────┘ └─────────────┘  │
└───────┬──────────────────────────────┬──────────────────────┘
        │                              │
   SQLite (WAL)                   ┌────▼─────────────────────┐
   görev/mesaj/iz                 │  ADAPTÖR KATMANI         │
   .divan/artifacts/  dosyalar    │  claude · codex ·        │
                                  │  copilot · agy · opencode │
                                  └────┬─────────────────────┘
                                       │ iki teslim yolu:
                    ┌──────────────────┴──────────────────┐
                    ▼                                     ▼
        HOOK ENTEGRASYONU (push)              MCP SERVER (ajan inisiyatifi)
        tur arası enjeksiyon,                 send_message, delegate_task,
        idle wake, aktivite kaydı             claim_task, publish_artifact,
                                              get_artifact, subscribe
```

### 3.1 Bileşen sorumlulukları

**Hub Daemon** — tek tokio süreci. Tüm durum SQLite'tadır (WAL modu). Arayüzler ve adaptörler unix socket üzerinden JSON-RPC ile konuşur.

- **Registry:** AgentCard kayıtları. Bir araç ilk kez `divan claude` ile başlatıldığında adaptör kendini kaydeder: yetenekler, model bilgisi, maliyet sınıfı, capability seti, desteklenen teslim yolları.
- **Task Scheduler:** Task durum makinesi + bağımlılık grafı (DAG). Bir task `done` olduğunda blokesi kalkan task'lar `open`'a çekilir, abone ajanlar tetiklenir. Tamamen deterministiktir (K2).
- **Message Bus:** Pointer tabanlı mesajlar (K4), teslim politikası (K3/K5), abonelik eşleştirme (K6). Teslim edilmemiş mesajlar ajan bazında kuyrukta bekler.
- **Policy Engine:** Her eylem (spawn, kill, write, delegate) capability kontrolünden geçer (K8). İhlaller trace'e yazılır ve reddedilir.
- **Cost Router:** Görev sınıfı → ajan skoru (K9). v1: YAML kural dosyası. v1.5: trace telemetrisiyle skor düzeltme.
- **Artifact Store:** `.divan/artifacts/` altında content-addressed dosyalar (BLAKE3 hash → yol). Metadata SQLite'ta. Mesajlar yalnızca hash + özet taşır.
- **Worktree Manager:** `git worktree add` / temizlik / diff özetleme. Task'a worktree ataması, merge-onay akışı.
- **Trace Collector:** Span/event kayıtları. `divan trace <task-id>` ile zaman çizelgesi; v1.5'te statik HTML export.

### 3.2 Adaptör katmanı

Her adaptör ortak trait'i implemente etmelidir:

```rust
#[async_trait]
trait AgentAdapter {
    fn card(&self) -> AgentCard;                       // yetenek + teslim yolu beyanı
    async fn spawn(&self, task: &Task, ctx: SpawnCtx) -> Result<SessionHandle>;
    async fn deliver(&self, h: &SessionHandle, batch: MessageBatch) -> Result<DeliveryReceipt>;
    async fn observe(&self, h: &SessionHandle) -> EventStream;  // normalize edilmiş olaylar
    async fn resume(&self, session_id: &str, prompt: &str) -> Result<SessionHandle>;
    async fn kill(&self, h: &SessionHandle) -> Result<()>;
}
```

**Olay normalizasyonu:** Her adaptör, aracın kendi çıktı formatını ortak şemaya çevirir: `ToolCall`, `FileEdit`, `TurnEnd`, `SessionIdle`, `SessionEnd`, `Error`. Hub yalnızca normalize olay görür. Yeni araç eklemek = yeni adaptör yazmak; çekirdek değiştirilmez.

**Hata taksonomisi:** `Error` olayı zorunlu bir sınıf alanı taşır: `retryable` (rate limit, geçici ağ/araç hatası) veya `fatal` (auth eksik, CLI bulunamadı, parse çöküşü, policy reddi). Scheduler'ın retry kararı bu sınıfa göre deterministik verilir (K2): `retryable` → backoff'lu sınırlı yeniden deneme (varsayılan 2), `fatal` → `failed` geçişi + insan bildirimi. Sınıflandıramayan adaptör `fatal` varsaymalıdır.

### 3.3 Araç uyumluluk matrisi

| Araç                    | Spawn (headless) | Akış çıktısı                     | Oturum devamı                                                                        | Uygunluk                                                                         |
| ----------------------- | ---------------- | -------------------------------- | ------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------- |
| Claude Code             | `claude -p`      | `--output-format stream-json` ✅ | `-r <session-id>` ✅                                                                 | **TAM** — referans adaptör                                                       |
| Codex                   | `codex e`        | `--json` ✅                      | `resume` ✅                                                                          | **TAM**                                                                          |
| Copilot CLI             | `copilot -p`     | düz metin (stream-json yok) ⚠️   | oturum devamı var; programatik kullanımı Faz 0'da doğrulanacak ⚠️                    | **UYGUN (kısıtlarla)** — bkz. 3.4                                                |
| Antigravity CLI (`agy`) | `agy -p`         | düz metin ⚠️                     | `-c` yalnız global son konuşma; `--conversation <id>` var ama `-p` ID döndürmüyor ❌ | **KISITLI** — bkz. 3.5                                                           |
| OpenCode                | stdin modu       | oturum API                       | var                                                                                  | **TAM (doğrulanacak)**                                                           |
| Gemini CLI              | —                | —                                | —                                                                                    | **KULLANILMAYACAK** — 18 Haziran 2026'da kapatılıyor; halefi Antigravity CLI'dır |

### 3.4 Copilot CLI uygunluk değerlendirmesi

**Sonuç: UYGUN — birinci sınıf adaptör adayı, bir kısıtla.**

Lehte bulgular:

- Programatik mod olgun ve resmi olarak desteklidir: `copilot -p "<prompt>"` tek atışta çalışır, yanıtı stdout'a yazar ve temiz çıkar; `-s/--silent` model metadata'sını kırpar. GitHub bu modu açıkça script, CI/CD ve otomasyon iş akışları için tasarlamıştır ve GitHub Actions ile otomasyon resmi dokümantasyonda örneklenmiştir.
- **`--allow-tool` ince taneli izin filtreleri** (shell/write/url/MCP araç türlerine parantezli desen filtreleri) sunar. Bu, Divan'ın Policy Engine'i (K8) ile doğrudan eşleşir: hub, task'ın capability setini spawn anında `--allow-tool` desenlerine çevirebilir. Bu, K8'in araç tarafında _native_ uygulanabildiği tek adaptördür — değerlendirmedeki en güçlü artı.
- `--model` ile model sabitleme Cost Router (K9) ile uyumludur.
- MCP server desteği vardır → Divan MCP yüzü (3. teslim yolu) kaydedilebilir.

Kısıtlar:

- Yapılandırılmış akış çıktısı (stream-json eşleniği) doğrulanamamıştır; çıktı düz metindir. Adaptör, olay normalizasyonunu akıştan değil şu kaynaklardan yapmalıdır: (a) nihai stdout metni, (b) worktree'deki dosya diff'leri, (c) varsa log dosyaları. `FileEdit` olayları diff'ten türetilir; `ToolCall` granülaritesi v1'de bu adaptörde eksik kalabilir — kabul edilebilir.
- Hook sistemi Claude Code'unki kadar zengin değildir; teslim yolu önceliği bu adaptörde MCP > idle-resume olarak işaretlenmelidir.

**Adaptör kararı:** Faz 3'te 3. adaptör olarak Copilot CLI tercih edilir (OpenCode yerine öne alınabilir); `--allow-tool` ↔ Policy Engine eşlemesi Faz 3'ün gösterim senaryosuna dahil edilmelidir.

### 3.5 Antigravity CLI (`agy`) uygunluk değerlendirmesi

**Sonuç: KISITLI UYGUN — v1'de yalnızca tek atışlık (one-shot) görevler için degraded adaptör; tam adaptör upstream düzeltme beklemektedir.**

Lehte bulgular:

- Non-interaktif mod vardır: `agy -p/--print/--prompt` tek prompt'u çalıştırıp yanıtı basar; `--print-timeout` (varsayılan 5 dk), `--model` seçimi, `--add-dir` ile workspace genişletme, `--dangerously-skip-permissions` mevcuttur. Google bu modu script/CI otomasyonu için resmi olarak belgelemiştir.
- Kademeli izin/sandbox modları vardır (salt okunur kısıtlamadan, izole konteynerde otomatik komut çalıştırmaya, tam otonom moda kadar) — Policy Engine ile kavramsal uyum iyidir.
- MCP server desteği vardır → Divan MCP yüzü kaydedilebilir.
- Gemini CLI'ın halefi olduğu için stratejik olarak önemlidir; Gemini kullanıcı tabanı bu araca taşınmaktadır.

Engelleyici kısıtlar:

- **Oturum kimliği problemi (kritik):** `agy -p` çalıştırması, oluşturduğu konuşmanın ID'sini stdout/stderr/dosya hiçbir yerde döndürmemektedir. `--conversation <id>` ile belirli oturuma devam mümkündür ama ID'yi yakalamanın yolu yoktur; `-c/--continue` ise makinedeki _global en son_ konuşmayı devam ettirir — birden fazla bağımsız oturum yöneten bir orkestratör için bu kırıktır (upstream issue: google-antigravity/antigravity-cli#7). Aynı sorun diğer wrapper projelerini de (Vyzr, Crosstalk) engellemektedir.
- Yapılandırılmış akış çıktısı yoktur; `--output-format json` denemeleri hata vermektedir.
- Asenkron subagent özelliği yalnızca interaktif TUI içindedir (`/agent`), headless yüzeyde yoktur.

**Adaptör kararı:** v1'de `AgyAdapter` yalnızca durumsuz, tek atışlık görev türlerini kabul eder (`review`, `research`, `analyze`, `report`); `resume` çağrısı `Unsupported` döner ve Card'da beyan edilir. Çok turlu görevler Cost Router tarafından bu ajana atanmaz (router kuralı: `match: {multi_turn: true} exclude: [agy]`). Upstream issue #7 çözüldüğünde tam adaptöre yükseltilir; Faz 0/S5 spike'ı issue'nun durumunu kontrol etmeli ve `-p` çıktısının konuşma kayıt dosyalarını diskte bırakıp bırakmadığını (geçici çözüm potansiyeli) test etmelidir.

### 3.6 İki teslim yolu (K1)

**Yol A — Hook (hub → ajan, push):** Kurulumda `divan install-hooks`, destekleyen araçların config dizinlerine ince hook script'leri yazar. Hook'lar: (a) aktiviteyi hub'a raporlar, (b) tur sınırında bekleyen MessageBatch'i context'e enjekte eder, (c) boştaki oturumu uyandırır. Divan kullanılmıyorsa hook'lar no-op olmalıdır. Hook kurulumu kullanıcının mevcut hook config'ini **ezmez**; mevcut girdilerle birleştirir (merge) ve kurulum öncesi yedek alır.

**Enjeksiyon bloğu davranış yönergesi:** Enjekte edilen her `[DIVAN MESSAGES]` bloğunun başında sabit, tek satırlık bir talimat bulunur: bu bloğun kullanıcı mesajı olmadığı, yanıtların yalnızca `send_message` MCP tool'u ile verileceği ve bloğa context içinde serbest metinle cevap yazılmayacağı belirtilir. Bu yönerge olmadan ajan, enjeksiyonu sohbet sanıp context'te yanıt üreterek token sızdırır (K3/K4 ihlali).

**Yol B — MCP (ajan → hub, inisiyatif):** Hub bir MCP server yüzü açar; her araca kaydedilir. Tool seti minimal ve token-bilinçli tutulmalıdır (tool tanımları da her turda context'tedir):

- `send_message(to, kind, summary, artifact_ref?)` — özet ≤400 karakter, şemada zorunlu
- `delegate_task(kind, spec_artifact, constraints)` — spec dosyada, mesajda referans
- `claim_task(filter)` / `complete_task(id, result_artifact, unblocks?)`
- `publish_artifact(path | content) → ref` / `get_artifact(ref) → içerik`
- `subscribe(event_filter)` / `list_agents(capability_filter)`

Teslim yolu önceliği (adaptör Card'da beyan eder): hook > MCP push > idle-wake'te resume + prompt.

### 3.7 Daemon yaşam döngüsü, watchdog ve kurtarma

**Tekillik:** Daemon başlarken lock file alır (`~/.local/share/divan/divan.lock`); ikinci daemon başlatılamaz, CLI mevcut daemon'a yönlendirir.

**Çökme/yeniden başlama (startup reconciliation):** Daemon her açılışta DB'deki aktif kayıtları gerçek dünyayla uzlaştırır: kayıtlı PID/session'ların hayatta olup olmadığı kontrol edilir; ölü session'a bağlı `claimed|working|review` task'ları `failed` durumuna çekilir (trace sebebi: `orphaned`), ilgili ajan kayıtları `offline` yapılır. v1 kararı: daemon ölümünde hayatta kalan ajan süreçlerine yeniden bağlanma (re-attach) **yapılmaz**; bu süreçler reconciliation'da sonlandırılır. Re-attach v2 adayıdır.

**Watchdog:** Her aktif session bir zamanlayıcıyla izlenir. Task'ın `max_runtime_secs` değeri (yoksa config'teki kind-bazlı varsayılan; örn. `implement: 1800`, `review: 900`) aşılırsa session `kill` edilir ve task `failed` olur (trace sebebi: `timeout`). Watchdog tetiklemeleri policy kontrolünden muaftır (hub'ın kendi yetkisidir) ama trace'e yazılır.

**Artifact çöp toplama:** `divan cleanup <task-id>` yalnızca worktree'yi siler; artifact'lara **dokunmaz** (content-addressed depo referans sayımı gerektirir). `divan gc` komutu v2 kapsamındadır; v1 README'sinde `.divan/artifacts/` dizininin büyüyebileceği "known limitations" altında belirtilmelidir.

---

## 4. Veri Modeli (SQLite şeması)

```sql
-- Ajan kayıtları (A2A AgentCard'dan uyarlama)
CREATE TABLE agents (
  id            TEXT PRIMARY KEY,        -- "claude-1", "codex-rev"
  tool          TEXT NOT NULL,           -- claude|codex|copilot|agy|opencode
  display_name  TEXT,
  capabilities  TEXT NOT NULL,           -- JSON: ["read","write","delegate"]
  cost_class    INTEGER NOT NULL,        -- 1 ucuz .. 5 pahalı
  skills        TEXT,                    -- JSON: ["csharp","review","sql"]
  delivery      TEXT NOT NULL,           -- JSON: ["hook","mcp","resume"]
  multi_turn    INTEGER NOT NULL,        -- 0|1 (bkz. 3.5: agy=0)
  status        TEXT NOT NULL,           -- idle|busy|offline
  session_id    TEXT,
  registered_at INTEGER NOT NULL
);

-- Görevler (A2A Task yaşam döngüsü)
CREATE TABLE tasks (
  id           TEXT PRIMARY KEY,
  parent_id    TEXT REFERENCES tasks(id),
  kind         TEXT NOT NULL,            -- implement|review|test|plan|research
  title        TEXT NOT NULL,
  spec_ref     TEXT,                     -- artifact referansı (K4)
  state        TEXT NOT NULL,            -- open|claimed|working|review|done|failed|cancelled
  assignee     TEXT REFERENCES agents(id),
  worktree     TEXT,                     -- path veya NULL (salt okunur görev)
  max_runtime_secs INTEGER,              -- NULL = kind bazlı config varsayılanı (watchdog)
  trace_id     TEXT NOT NULL,
  created_at   INTEGER, updated_at INTEGER
);

CREATE TABLE task_deps (
  task_id    TEXT REFERENCES tasks(id),
  blocked_by TEXT REFERENCES tasks(id),
  PRIMARY KEY (task_id, blocked_by)
);

-- Mesajlar: pointer + özet, içerik YOK (K4)
-- Fan-out kuralı (P0.2): to_agent=NULL mesajlar teslim kuyruğuna girmez;
-- abonelik eşleşmesinde hedef başına ayrı satır kopyalanır (origin_message_id ile bağlı).
CREATE TABLE messages (
  id                TEXT PRIMARY KEY,
  origin_message_id TEXT REFERENCES messages(id), -- fan-out kopyası ise kaynak yayın
  from_agent   TEXT NOT NULL,
  to_agent     TEXT,                     -- NULL = yalnız kaynak yayın satırında
  kind         TEXT NOT NULL,            -- handoff|review_done|question|status|alert
  summary      TEXT NOT NULL CHECK(length(summary) <= 400),
  payload      TEXT,                     -- küçük yapılandırılmış JSON
  artifact_ref TEXT,                     -- ağır içerik buraya
  task_id      TEXT, trace_id TEXT,
  created_at   INTEGER,
  delivered_at INTEGER                   -- NULL = kuyrukta (batching, K5); receipt sonrası yazılır
);

CREATE TABLE artifacts (
  ref        TEXT PRIMARY KEY,           -- blake3 hash
  path       TEXT NOT NULL,              -- .divan/artifacts/ab/cdef...
  mime       TEXT, bytes INTEGER,
  summary    TEXT,                       -- üreticinin yazdığı ≤400 karakter
  created_by TEXT, created_at INTEGER
);

CREATE TABLE subscriptions (
  agent_id TEXT, event_kind TEXT, filter TEXT,
  PRIMARY KEY (agent_id, event_kind)
);

-- Gözlemlenebilirlik (K10)
CREATE TABLE trace_events (
  id        INTEGER PRIMARY KEY,
  trace_id  TEXT NOT NULL, span_id TEXT, parent_span TEXT,
  agent_id  TEXT, event TEXT NOT NULL,   -- spawn|tool_call|file_edit|msg_sent|policy_denied|...
  data      TEXT,                        -- JSON detay
  ts        INTEGER NOT NULL
);

-- Çakışma tespiti (hcom'un 30 sn penceresi deseni)
CREATE TABLE file_touches (
  path TEXT, agent_id TEXT, task_id TEXT, ts INTEGER
);
```

**Task durum makinesi:**
`open → claimed → working → review → done` · yan çıkışlar: `failed`, `cancelled` · `review → working` döngüsü serbesttir. Durum geçişleri yalnızca hub kodundan yapılır; her geçiş bir trace_event üretir. Adaptörler veya ajanlar doğrudan SQLite'a yazamaz.

---

## 5. Farklılaştırıcı Modüller

### 5.1 Policy Engine (K8)

- Capability seti kayıt anında belirlenir; çalışma anında yalnızca insan (CLI üzerinden) genişletebilir.
- Eylem matrisi: `spawn` → hedefin cost_class'ı çağıranın limitini aşamaz; `kill` → yalnız kendi spawn ettiği oturumlar; `write` → yalnız atanmış worktree içinde (hook PreToolUse path kontrolü; Copilot adaptöründe `--allow-tool write(...)` desenine derlenir); `broadcast` → varsayılan kapalı.
- Her red bir `policy_denied` trace olayı üretir: kim, neyi, neden.

### 5.2 Cost Router (K9)

- v1 girdileri: `task.kind`, spec uzunluğu, `agents.cost_class`, `agents.skills`, `agents.multi_turn`, anlık `status`.
- v1 kural dosyası örneği:
  ```yaml
  rules:
    - match: { kind: test }          prefer: { cost_class: "<=2" }
    - match: { kind: review }        prefer: { skills: [review], different_vendor_than: author }
    - match: { kind: plan }          prefer: { cost_class: ">=4" }
    - match: { multi_turn: true }    exclude: { tool: agy }   # bkz. 3.5
  ```
- v1.5: trace verisinden ajan×görev-türü başarı oranı ve ortalama maliyet → skor düzeltme.

### 5.3 Trace Collector (K10)

- Trace = bir kök Task'ın tüm yaşamı. Span = bir ajan oturumu/etkileşimi. Mesajlar trace_id taşır.
- `divan trace <id>`: TUI zaman çizelgesi (ajan şeritleri, mesaj okları, durum geçişleri); v1.5: tek dosyalık statik HTML export.
- Ölçülen metrikler: mesaj başına ortalama özet uzunluğu, oturum başına enjeksiyon sayısı, görev başına maliyet sınıfı dağılımı. README'deki "token-bilinçli mimari" iddiası bu sayılarla desteklenmelidir; yüzde tasarruf iddiası yapılmamalıdır.

---

## 6. Dil ve Teknoloji Kararı

**Karar: Rust (hub + adaptörler + CLI/TUI), tek cargo workspace.**

Gerekçe: daemon + süreç yönetimi + tek-binary dağıtım Rust'ın güçlü alanıdır; referans proje hcom da Rust'tır. Proje sahibinin Tokio/Axum/SQLx aşinalığı mevcuttur.

Crate seti: `tokio` (runtime), `serde`/`serde_json`, `rusqlite` (lokal SQLite için sqlx'ten hafif), `clap` (CLI), `ratatui` (TUI), `rmcp` (resmi Rust MCP SDK), `notify` (dosya izleme), `blake3`, `tracing` + `tracing-subscriber`.

B planı: Faz 0 sonunda Rust ilerleme hızı yetersiz bulunursa çekirdek C# / .NET 8'e (BackgroundService + resmi C# MCP SDK) taşınır; hook script'leri her durumda Python/shell kalır. Karar Faz 0 sonunda kilitlenir ve sonrasında değiştirilmez. ADR 0001'e şu kural yazılmalıdır: **B planına geçiş yalnızca Faz 0 kabul kapısında tetiklenebilir; Faz 1 başladıktan sonra dil değişikliği yapılamaz.**

**Platform kapsamı (P0.4):** v1 hedefi macOS + Linux'tur (unix socket, POSIX shell hook script'leri). Windows desteği (named pipe, PowerShell hook'ları) v2 kapsamındadır ve v1'de Windows için soyutlama yazılmaz.

---

## 7. Yol Haritası

Her fazın sonunda çalışan, gösterilebilir bir çıktı olmalıdır (demo-driven). Faz kabul kriterleri sözleşmedir.

### Faz 0 — Doğrulama Spike'ları (1.–2. hafta)

Amaç: riskli varsayımları throwaway kodla doğrulamak. Spike kodu üretim koduna kopyalanmaz.

- S1: `claude -p --output-format stream-json` çıktısını Rust'ta parse et; olay normalizasyonu ilk taslağı.
- S2: Aynısı `codex e --json` için; iki olay modelini yan yana belgele.
- S3: Hook enjeksiyonu kanıtı: Claude Code hook'uyla çalışan oturuma tur arası metin enjekte et; Stop hook'unda hub'a sinyal gönder.
- S4: `rmcp` ile 2 tool'luk oyuncak MCP server; Claude Code'a kaydet, tool çağrısı gidiş-dönüşünü doğrula.
- S5: `copilot -p` ve `agy -p` davranış testleri: çıktı formatı, exit kodları, `--allow-tool` desen derlemesi (Copilot), konuşma kayıt dosyalarının diskte aranması ve issue #7 durum kontrolü (agy).
- **Kabul:** 5 kısa spike raporu (md) + dil kararının kilitlenmesi + 3.3 matrisinin doğrulanmış sürümü.

### Faz 1 — MVP: "Yaz → Review → Rapor" (3.–6. hafta)

Tek killer akış uçtan uca: _Claude implement eder, Codex review eder, anlaşmazlık raporu insana gider._

- Hub iskeleti: daemon, unix socket JSON-RPC, SQLite şema.
- Daemon yaşam döngüsü (3.7): lock file, startup reconciliation (`orphaned` geçişleri), temiz kapanış.
- Session watchdog (3.7): `max_runtime_secs` + kind-bazlı varsayılanlar, `failed(timeout)` geçişi.
- ClaudeAdapter + CodexAdapter (spawn, observe, resume, kill) — `Error{retryable|fatal}` taksonomisi ve scheduler retry politikası dahil.
- Task Scheduler v1: lineer bağımlılık, durum makinesi.
- Artifact Store + pointer mesaj (K4) zorunluluğu.
- CLI: `divan up`, `divan run "<görev>" --flow write-review`, `divan status`, `divan log`.
- Worktree Manager v1: task başına worktree, `divan diff`, `divan merge`; `divan cleanup` artifact'lara dokunmaz.
- **Kabul:** Gerçek bir repoda (hedef: proje sahibinin FinBoard reposu) akış komutla çalışır; anlaşmazlık raporu artifact olarak üretilir; demo GIF kaydedilir; daemon kill edilip yeniden başlatıldığında reconciliation öksüz task'ları `failed(orphaned)` yapar; takılı sahte adaptör watchdog ile `failed(timeout)` olur.

### Faz 2 — Mesajlaşma Çekirdeği (7.–9. hafta)

- Hook paketi: `divan install-hooks` (Claude + Codex), aktivite kaydı, tur arası enjeksiyon, idle wake; kurulum mevcut kullanıcı hook'larıyla birleşir (merge), asla ezmez, yedek alır.
- MCP server yüzü: 3.6-B'deki tool seti, policy kontrolünden geçerek.
- Batching (K5) + abonelik (K6) + teslim makbuzları; fan-out Seçenek A ile (hedef başına satır, `origin_message_id` bağlı — P0.2).
- Enjeksiyon bloğu davranış yönergesi (3.6 Yol A) batch formatına dahil edilir.
- Çakışma tespiti (file_touches, 30 sn penceresi).
- **Kabul:** İki ajan MCP üzerinden soru sorup hook ile cevap alabilir; tüm trafik `divan log`'da görünür; mesaj başına özet uzunluğu raporlanır; fan-out teslimleri trace'te kaynak yayına bağlı izlenir.

### Faz 3 — İzin + Maliyet + 3. Adaptör (10.–12. hafta)

- Policy Engine: capability matrisi, PreToolUse path kontrolü, policy_denied trace'leri.
- Cost Router v1: YAML kurallar, `different_vendor_than: author` ve `exclude: agy (multi_turn)` dahil.
- 3. adaptör: **CopilotAdapter** (`--allow-tool` ↔ Policy Engine derlemesi gösterim senaryosudur). 4. adaptör adayı: AgyAdapter (degraded, one-shot).
- **Kabul:** Yetkisiz `kill` denemesi reddedilir ve izlenir; `--flow write-review` reviewer'ı otomatik farklı vendor'dan seçer; Copilot'a verilen task'ta yazma izni worktree path'ine `--allow-tool` ile daraltılmıştır.

### Faz 4 — Gözlemlenebilirlik + TUI (13.–15. hafta)

- `divan` TUI (ratatui): ajan panoları, görev panosu, canlı mesaj akışı.
- `divan trace <id>`: metin/TUI zaman çizelgesi (v1.0 kapsamı — P0.1 kararı).
- Statik HTML export (`divan trace export --format html`) v1.5 kapsamındadır; Faz 4'te opsiyonel feature flag arkasında denenebilir ama **kabul kriteri değildir**.
- Token metrikleri panosu.
- **Kabul:** Tek ekrandan 3 ajanlı bir akışın tamamı izlenebilir; `divan trace <id>` metin zaman çizelgesi üretir; metrikler (özet uzunluğu, enjeksiyon sayısı, maliyet sınıfı dağılımı) raporlanır.

### Faz 5 — Cila ve Lansman (16.–18. hafta)

- README (İngilizce, mimari diyagram, GIF'ler), kurulum: `cargo install` + shell installer.
- CI: GitHub Actions (test, clippy, Linux/macOS çapraz derleme).
- Dokümantasyon: K1–K10 için ADR dosyaları, CONTRIBUTING.
- Lansman: Show HN, r/ClaudeCode, awesome-agent-orchestrators listesine PR, LinkedIn yazı dizisi (İngilizce).
- **Kabul:** Dış bir kullanıcı 10 dakikada kurup MVP akışını çalıştırabilir.

### v2 ufku (kapsam dışı — yalnızca README roadmap'inde listelenir)

A2A gateway (uzak ajanlar), web trace UI, çok-kullanıcılı mod, telemetri tabanlı router öğrenmesi, fonksiyon-seviyesi kilit (Tree-sitter), AgyAdapter'ın tam adaptöre yükseltilmesi (upstream issue #7 sonrası), Windows desteği (named pipe + PowerShell hook'ları — P0.4), `divan gc` artifact çöp toplama (referans sayımı), daemon re-attach (ölümde hayatta kalan session'lara yeniden bağlanma), `message_deliveries` tablosuna geçiş (P0.2 Seçenek B — receipt ihtiyacı büyürse migration).

---

## 8. Riskler ve Önlemler

| Risk                                          | Olasılık | Önlem                                                                                            |
| --------------------------------------------- | -------- | ------------------------------------------------------------------------------------------------ |
| Araç CLI bayrakları/hook API'leri değişir     | Yüksek   | Adaptör izolasyonu; sürüm tespiti + uyumluluk testleri; CI'da haftalık smoke test.               |
| İlk büyük Rust projesi yavaşlatır             | Orta     | Faz 0 doğrulaması + B planı (C#); "önce clone, sonra optimize" pragmatizmi.                      |
| Hook enjeksiyonu bazı araçlarda desteklenmez  | Orta     | Teslim hiyerarşisi (3.6); adaptör desteğini Card'da beyan eder.                                  |
| agy oturum kimliği sorunu çözülmez            | Orta     | Degraded one-shot adaptör tasarımı sorunu baştan kabullenmiştir; tam adaptör v2'ye ertelidir.    |
| Daemon çökmesi aktif oturumları öksüz bırakır | Orta     | Startup reconciliation + lock file (3.7); v1'de re-attach yok, dürüst `failed(orphaned)` geçişi. |
| Takılan ajan sistemi süresiz kilitler         | Orta     | Watchdog + kind-bazlı `max_runtime` varsayılanları (3.7).                                        |
| Kapsam şişmesi                                | Yüksek   | Faz kabul kriterleri sözleşmedir; yeni fikir = v2 listesine not, koda değil.                     |
| Benzer projeler hızla gelişir                 | Kesin    | Farklılaşma üç boşluktadır (izin, maliyet, trace); portföy değeri rekabetten bağımsızdır.        |
| Token metriği kaba kalır                      | Düşük    | Karakter-bazlı tahmin v1 için yeterli; yüzde tasarruf iddiası yapılmaz.                          |

---

## 9. Başarı Ölçütleri

**Teknik:** MVP akışı gerçek repoda <5 dk kurulumla çalışır · mesaj başına ortalama özet ≤400 karakter · koordinasyon yolunda sıfır LLM çağrısı (tasarımla garanti) · 3+ araç adaptörü · policy_denied ve trace olayları uçtan uca doğrulanmış.

**Proje sahibi hedefleri:** 10 dakikalık canlı demo + K1–K10 karar anlatısı · İngilizce README + 4-5 teknik LinkedIn yazısı · dış kullanıcıdan ilk issue/star.

---

## 10. İlham Kaynakları ve Referans Projeler

Belirsizlik durumunda önce bu projelerin ilgili implementasyonu incelenmelidir.

**Doğrudan ilham alınan projeler (alınan desen ile):**

| Proje          | Link                                                         | Divan'a katkısı                                                                                                                                                                                    |
| -------------- | ------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| hcom           | https://github.com/aannoo/hcom                               | Hook tabanlı teslimat (tur arası enjeksiyon, idle wake), SQLite mesaj omurgası, 30 sn çakışma penceresi, Rust referansı. Güvenlik modeli _karşıt örnek_: total-trust yaklaşımı K8'in gerekçesidir. |
| myclaude       | https://github.com/cexll/myclaude                            | Adaptör reçetesi: araç başına headless bayrak haritası (`codex e --json`, `claude --output-format stream-json -r` vb.), orkestratör/executor ayrımı.                                               |
| swarm-protocol | https://github.com/phuryn/swarm-protocol                     | "Koordinasyon katmanı = MCP server" deseni; claim/complete/unblocks görev akışı. Auth eksikliği karşıt örnek (K8).                                                                                 |
| bernstein      | https://github.com/chernistry/bernstein                      | "Koordinasyonda sıfır LLM token'ı" ilkesi (K2), test-doğrulamalı deterministik orkestrasyon.                                                                                                       |
| A2A Protocol   | https://github.com/a2aproject/A2A · https://a2a-protocol.org | AgentCard/Task/Message/Artifact veri modeli (K1), task yaşam döngüsü durumları.                                                                                                                    |
| crystal        | https://github.com/stravu/crystal                            | Git worktree izolasyon deseni (K7), paralel oturum yönetimi.                                                                                                                                       |
| guild          | https://github.com/mathomhaus/guild                          | Tek binary + lokal SQLite + paylaşılan bağlam yaklaşımı.                                                                                                                                           |
| shire          | https://github.com/victor36max/shire                         | Ajanlar arası mailbox deseni, kalıcı çalışma alanları.                                                                                                                                             |
| gnap           | https://github.com/farol-team/gnap                           | Orkestratörsüz git-native görev panosu (minimalizm dersi; Divan bilinçli olarak daemon'lu yolu seçer).                                                                                             |
| wit            | https://github.com/amaar-mc/wit                              | Fonksiyon seviyesi kilit (Tree-sitter AST) — v2 adayı.                                                                                                                                             |
| kodo           | https://github.com/ikamensh/kodo                             | Bağımsız doğrulamalı iş döngüleri (Faz 1 write-review akışının esini).                                                                                                                             |
| vibe-kanban    | https://github.com/BloopAI/vibe-kanban                       | Ajan-agnostik görev panosu UX'i (TUI tasarımına referans).                                                                                                                                         |

**Ekosistem haritası:** https://github.com/andyrewlee/awesome-agent-orchestrators

**Araç dokümantasyonu:**

- Claude Code headless: https://docs.anthropic.com/en/docs/claude-code/sdk/sdk-headless
- Copilot CLI programatik referans: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-programmatic-reference
- Copilot CLI + GitHub Actions: https://docs.github.com/en/copilot/how-tos/copilot-cli/automate-with-actions
- Antigravity CLI oturum ID sorunu (takip edilmeli): https://github.com/google-antigravity/antigravity-cli/issues/7

---

## 11. Karar Kayıtları (implementasyon planı P0 soruları)

`implementation_plan.md` Bölüm 2'de açılan sorulara spec kararları:

| #    | Soru                      | Karar                                                                                                                                                                                                                                                                                                               |
| ---- | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| P0.1 | Trace HTML export kapsamı | v1.0 = `divan trace <id>` metin/TUI timeline. HTML export (`divan trace export --format html`) v1.5. Faz 4'te opsiyonel feature flag ile denenebilir, kabul kriteri sayılmaz.                                                                                                                                       |
| P0.2 | Çok alıcılı mesaj teslimi | **Seçenek A**: abonelik fan-out'unda hedef başına ayrı `messages` satırı; teslim kuyruğunda `to_agent` asla NULL kalmaz. Kopyalar `origin_message_id` ile kaynak yayına bağlanır (trace izlenebilirliği). `message_deliveries` tablosu (Seçenek B) receipt ihtiyacı büyürse v1.5+ migration olarak değerlendirilir. |
| P0.3 | Dil kararı kilidi         | Onaylandı. ADR 0001'e ek kural: B planına (C#) geçiş yalnızca Faz 0 kabul kapısında tetiklenebilir; Faz 1 sonrası dil değişikliği yapılamaz.                                                                                                                                                                        |
| P0.4 | İşletim sistemi sınırı    | v1 = macOS + Linux (unix socket, POSIX shell hook'ları). Windows (named pipe, PowerShell) v2; v1'de Windows soyutlaması yazılmaz.                                                                                                                                                                                   |

---

## 12. Değişiklik Günlüğü

**v0.3 (Haziran 2026)** — Codex implementasyon planı review'u sonrası:

- P0.1–P0.4 kararları eklendi (Bölüm 11); Faz 4 kabul kriterinden HTML export çıkarıldı (v0.2'deki kapsam/Faz 4 tutarsızlığı giderildi).
- Yeni Bölüm 3.7: daemon lock file, startup reconciliation (`failed(orphaned)`), session watchdog (`max_runtime_secs`, `failed(timeout)`), artifact GC v1 sınırı (`cleanup` artifact'lara dokunmaz; `gc` v2).
- Olay modeline hata taksonomisi eklendi: `Error{retryable|fatal}` + deterministik retry politikası (3.2).
- Mesaj şemasına `origin_message_id` (fan-out Seçenek A), tasks şemasına `max_runtime_secs` eklendi.
- Enjeksiyon bloğu davranış yönergesi tanımlandı (3.6 Yol A); hook kurulumuna merge/yedek kuralı eklendi.
- Faz 1 ve Faz 2 iş listeleri ile kabul kriterleri yukarıdakilere göre genişletildi; risk tablosuna daemon çökmesi ve takılan ajan satırları eklendi; v2 ufku güncellendi.
- Bölüm 6'ya platform kapsamı (P0.4) ve B planı kilit kuralı (P0.3) işlendi; Bölüm 0'a eşlik dokümanı kuralı eklendi.

**v0.2** — Ajan-hedefli dil, ilham kaynakları (Bölüm 10), Copilot CLI ve Antigravity CLI uygunluk değerlendirmeleri (3.4, 3.5), Gemini CLI'ın kullanımdan kaldırılması.

**v0.1** — İlk mimari ve plan taslağı.
