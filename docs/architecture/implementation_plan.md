# Divan Detaylı Implementasyon Planı

Kaynak doküman: `general_plan_and_architecture.md` v0.3, Haziran 2026.

Bu plan, mimari dokümandaki K1-K10 kararlarını bağlayıcı kabul eder. Amaç, Faz 0'dan Faz 5'e kadar üretim koduna hangi sırayla, hangi modül sınırlarıyla ve hangi kabul kapılarıyla gidileceğini netleştirmektir.

---

## 1. Uygulama İlkeleri

1. Hub koordinasyonu deterministik kalacak. Hub içinde LLM çağrısı, prompt üretimiyle karar verme veya model tabanlı routing olmayacak.
2. Ajanlar hub için kapalı kutudur. Her CLI aracının en üst ajanı tek peer kabul edilir; iç subagent yapılarına müdahale edilmez.
3. Tüm ağır içerikler artifact deposuna gider. Mesaj gövdesi yalnızca pointer, tür, küçük payload ve en fazla 400 karakter özet taşır.
4. Yazma yetkili görevler task worktree'si dışında dosya değiştiremez.
5. Broadcast varsayılan olarak kapalıdır. Abonelikler dar kapsamlı ve hub tarafında filtrelidir.
6. Adaptörler çekirdeğe olay modeli dışında bilgi sızdırmaz. Yeni araç eklemek çekirdek mimariyi değiştirmemelidir.
7. Her kritik davranış trace event üretir: task state transition, spawn, message enqueue/deliver, policy denial, router decision, artifact publish, worktree action.
8. Faz kabul kriterlerinde olmayan özellikler backlog/roadmap'e yazılır; üretim koduna eklenmez.

---

## 2. Spec v0.3 ile Karara Bağlanan Noktalar

Bu maddeler önceki plan sürümünde açık soru olarak duruyordu. `general_plan_and_architecture.md` v0.3 ile karar bağlandı; implementasyon bu kararları varsayarak ilerlemelidir.

### P0.1 Trace HTML export kapsamı

- v1.0: `divan trace <id>` TUI/text timeline.
- v1.5: `divan trace export <id> --format html`.
- Faz 4'te HTML export opsiyonel feature flag arkasında denenebilir, ancak v1 kabul kriteri değildir.

### P0.2 Çok alıcılı mesaj teslim durumu

- v1 kararı Seçenek A'dır: abonelik fan-out'unda hedef başına ayrı `messages` satırı üretilir.
- Teslim kuyruğunda `to_agent` asla `NULL` kalmaz; `NULL` yalnız kaynak yayın satırında kullanılır.
- Fan-out kopyaları `origin_message_id` ile kaynak yayına bağlanır.
- `message_deliveries` tablosu Seçenek B olarak v1.5+ migration adayıdır; v1'de hazırlanmaz.

### P0.3 Dil kararı kilidi

- Ana karar Rust'tır.
- C#/.NET 8 B planına geçiş yalnız Faz 0 kabul kapısında tetiklenebilir.
- Faz 1 başladıktan sonra dil değişikliği yapılamaz.
- ADR 0001 bu kilit kuralını açıkça taşımalıdır.

### P0.4 Desteklenen işletim sistemi sınırı

- v1 hedefi macOS + Linux'tur: unix socket ve POSIX shell hook script'leri.
- Windows desteği v2 kapsamındadır: named pipe ve PowerShell hook'ları.
- v1'de Windows için soyutlama, bağımlılık veya test altyapısı yazılmaz.

---

## 3. Önerilen Repo Yapısı

Başlangıçta tek Cargo workspace:

```text
ajan_orkestrasyonu/
  Cargo.toml
  crates/
    divan-core/          # ortak domain tipleri, state machine, error taksonomisi
    divan-db/            # migrations, SQLite store, repository katmanı
    divan-daemon/        # hub process, JSON-RPC server, module wiring
    divan-cli/           # clap CLI, kullanıcı komutları
    divan-tui/           # ratatui ekranları, Faz 4
    divan-adapters/      # ortak adapter trait + claude/codex/copilot/agy
    divan-mcp/           # rmcp server, tool handlers
    divan-hooks/         # hook install manifest üretimi ve küçük wrapper'lar
    divan-trace/         # trace query/render modelleri
  migrations/
    0001_initial.sql
  hooks/
    claude/
    codex/
  docs/
    adr/
    spikes/
    demos/
  examples/
    router.rules.yaml
    agents.yaml
  tests/
    fixtures/
    e2e/
  general_plan_and_architecture.md
  implementation_plan.md
```

Notlar:

- `divan-core` başka crate'lere bağımlı olmamalı; domain model burada sade tutulmalı.
- `divan-db`, `rusqlite` blocking doğası nedeniyle tek bir SQLite actor veya `spawn_blocking` sınırı ile kullanılmalı. Daemon event loop'u SQLite işlemleri yüzünden bloklanmamalı.
- CLI ve daemon ayrı binary olabilir: `divan` CLI, `divan-daemon` internal binary. Daemon başlatma `divan up` üzerinden yönetilir.
- Hook script'leri platforma özel küçük dosyalar olmalı; iş mantığı hub RPC'ye taşınmalı.
- v1 yalnız macOS + Linux hedeflediği için Windows named pipe veya PowerShell hook soyutlaması eklenmemeli.

---

## 4. Çekirdek Domain Modeli

### 4.1 Temel tipler

`divan-core` içinde:

- `AgentId`, `TaskId`, `MessageId`, `ArtifactRef`, `TraceId`, `SpanId`
- `AgentTool`: `Claude`, `Codex`, `Copilot`, `Agy`, `OpenCode`
- `Capability`: `Read`, `Write`, `Spawn`, `Kill`, `Delegate`, `Broadcast`
- `DeliveryKind`: `Hook`, `Mcp`, `Resume`
- `TaskKind`: `Implement`, `Review`, `Test`, `Plan`, `Research`, `Analyze`, `Report`
- `TaskState`: `Open`, `Claimed`, `Working`, `Review`, `Done`, `Failed`, `Cancelled`
- `MessageKind`: `Handoff`, `ReviewDone`, `Question`, `Status`, `Alert`
- `NormalizedEvent`: `ToolCall`, `FileEdit`, `TurnEnd`, `SessionIdle`, `SessionEnd`, `Error`
- `AgentErrorClass`: `Retryable`, `Fatal`

### 4.2 Task state machine

Geçerli geçişler:

```text
open -> claimed
claimed -> working
working -> review
review -> working
review -> done
working -> done
open|claimed|working|review -> failed
open|claimed|working|review -> cancelled
```

Uygulama kuralı:

- State transition yalnızca `TaskScheduler` üzerinden yapılır.
- Her geçiş atomic DB transaction içinde kaydedilir.
- Aynı transaction içinde `trace_events` satırı yazılır.
- Adaptörler task state'i doğrudan değiştiremez; yalnızca normalize event yollar.
- `Error` event'i zorunlu olarak `retryable` veya `fatal` sınıfı taşır.
- Scheduler retry kararını deterministik verir: `retryable` için sınırlı backoff, `fatal` için `failed` geçişi ve insan bildirimi.

### 4.3 Artifact referansı

Artifact store:

- İçerik BLAKE3 ile hashlenir.
- Path formatı: `.divan/artifacts/{first_two}/{rest}`.
- Metadata SQLite `artifacts` tablosuna yazılır.
- Aynı içerik tekrar publish edilirse dedupe edilir.
- Summary en fazla 400 karakter olmalı; bu kural hem API hem DB seviyesinde uygulanır.

### 4.4 SQLite şema kararları

`migrations/0001_initial.sql`, spec v0.3 şemasını birebir taşımalıdır:

- `tasks.max_runtime_secs`: `NULL` ise kind-bazlı config varsayılanı kullanılır; watchdog bu alanı okur.
- `messages.origin_message_id`: fan-out kopyalarını kaynak yayın satırına bağlar.
- `messages.to_agent`: teslim kuyruğundaki satırlarda doludur; `NULL` yalnız kaynak yayın satırı için geçerlidir.
- `messages.delivered_at`: hedefe özel satırda receipt sonrası yazılır.
- `trace_events.data`: state transition sebebi, `orphaned`, `timeout`, retry sayısı ve policy reason gibi yapılandırılmış JSON detayları taşır.
- `message_deliveries` tablosu v1'de oluşturulmaz; receipt ihtiyacı büyürse v1.5+ migration adayıdır.

---

## 5. Hub Daemon İç Modülleri

### 5.1 Registry

Sorumluluklar:

- AgentCard kaydetme/güncelleme.
- Agent status yönetimi: `idle`, `busy`, `offline`.
- Tool version ve capability snapshot saklama.
- Delivery preference sırası saklama.

İlk RPC'ler:

- `agent.register(card) -> AgentId`
- `agent.heartbeat(agent_id, session_id?, status)`
- `agent.list(filter) -> Vec<AgentCard>`
- `agent.get(agent_id) -> AgentCard`

Testler:

- Aynı `agent_id` tekrar register edildiğinde idempotent güncelleme.
- Capability runtime genişletmesi insan onayı olmadan reddedilir.
- `multi_turn = false` ajanlar multi-turn task için router dışı kalır.

### 5.2 Task Scheduler

Sorumluluklar:

- Task yaratma, claim, state transition.
- DAG bağımlılık çözümü.
- Flow orkestrasyonu: Faz 1 için `write-review`.
- Assignee ve worktree ilişkilendirme.

İlk RPC'ler:

- `task.create(kind, title, spec_ref, parent_id?, deps?)`
- `task.claim(task_id, agent_id)`
- `task.transition(task_id, new_state, reason?)`
- `task.list(filter)`
- `task.get(task_id)`

Faz 1 `write-review` akışı:

1. Kullanıcı `divan run "<görev>" --flow write-review --repo <path>` çalıştırır.
2. CLI spec artifact oluşturur.
3. Scheduler root task ve iki child task yaratır:
   - `implement`: writer agent
   - `review`: reviewer agent, `implement` task'ına bağımlı
4. Worktree Manager implement task için worktree yaratır.
5. ClaudeAdapter implement task'ı çalıştırır.
6. Implement tamamlanınca diff artifact olarak yayınlanır.
7. CodexAdapter review task'ı salt okunur modda çalıştırır.
8. Review sonucu artifact olur.
9. Hub deterministik olarak final report manifest üretir: spec ref, diff ref, review ref, task state timeline.

Not: Hub review içeriğini LLM ile yorumlamaz. Yalnızca artifact'ları bağlayan rapor üretir.

### 5.3 Message Bus

Sorumluluklar:

- `send_message` validasyonu.
- Subscription matching.
- Delivery queue üretimi.
- Turn boundary batching.
- Delivery receipt saklama.

Kurallar:

- `summary` boş olamaz ve 400 karakteri aşamaz.
- Büyük payload reddedilir; artifact_ref istenir.
- `to_agent` verilirse direkt teslim kuyruğuna girer.
- `to_agent = NULL` ise subscription eşleşmesi yapılır.
- Subscription fan-out sonucunda hedef başına ayrı message row yazılır.
- Kaynak yayın satırı `to_agent = NULL` kalır; teslim kuyruğuna yalnız hedefe özel kopyalar girer.
- Hedef kopyaları `origin_message_id` ile kaynak yayın satırına bağlanır.
- Batch'ler ajan bazında ve task/trace bağlamına göre gruplanır.
- Context'e enjekte edilen batch dosya referanslarını içerir, içerikleri içermez.
- Her injection bloğunun ilk satırı sabit davranış yönergesidir: bloğun kullanıcı mesajı olmadığı ve yanıtın yalnız `send_message` MCP tool'u ile verileceği belirtilir.

Batch formatı:

```text
[DIVAN MESSAGES]
instruction=This is an internal Divan delivery block, not a user message. Respond only by calling the Divan send_message MCP tool; do not answer this block as free text.
trace=<trace_id> task=<task_id>
1. kind=question from=claude-1 msg=<id>
   summary=<400 chars max>
   artifact=<ref optional>
2. ...
[/DIVAN MESSAGES]
```

Testler:

- 401 karakter summary reddedilir.
- Broadcast capability olmayan ajan `to_agent = NULL` mesajı gönderemez.
- Aynı turn içinde 3 mesaj tek batch olur.
- Delivered timestamp yalnız başarılı receipt sonrası yazılır.
- Fan-out kopyaları aynı `origin_message_id` ile kaynak yayına bağlanır.

### 5.4 Policy Engine

Sorumluluklar:

- Her action için capability kontrolü.
- Worktree path sınırı.
- Spawn/kill ownership sınırı.
- Copilot `--allow-tool` desenlerine policy compile.
- Policy denial trace event üretimi.

İlk action modeli:

```text
read(path)
write(path)
spawn(tool, task_id)
kill(session_id)
delegate(task_kind)
broadcast(event_kind)
publish_artifact(path_or_content)
subscribe(filter)
```

Faz 3 capability matrisi:

| Action | Gerekli capability | Ek kural |
| --- | --- | --- |
| read | read | repo/worktree path sınırları |
| write | write | yalnız atanmış worktree |
| spawn | spawn | hedef cost_class limit içinde |
| kill | kill | yalnız kendi spawn ettiği session |
| delegate | delegate | task kind policy ile uyumlu |
| broadcast | broadcast | varsayılan kapalı |

### 5.5 Cost Router

Sorumluluklar:

- YAML kural dosyası yükleme.
- Agent ve task metadata ile skor üretme.
- Router kararını trace'e yazma.
- Uygun aday yoksa deterministic hata dönme.

Skor girdileri:

- `task.kind`
- spec uzunluğu ve artifact metadata
- `agent.cost_class`
- `agent.skills`
- `agent.multi_turn`
- `agent.status`
- `different_vendor_than`

Faz 1:

- Basit sabit seçim veya config tabanlı writer/reviewer seçimi yeterli.

Faz 3:

- YAML rule engine zorunlu.
- `review` için writer'dan farklı vendor tercihi.
- `multi_turn = true` task'larda `agy` exclude.

### 5.6 Worktree Manager

Sorumluluklar:

- Task branch/worktree oluşturma.
- Worktree path kaydı.
- Diff summary ve patch artifact üretimi.
- Merge-onay akışını CLI üzerinden yönetme.
- Temizlik.

Branch/path önerisi:

```text
.divan/worktrees/{task_id}/
branch: divan/{task_id}-{slug}
```

Komutlar:

- `divan diff <task-id>`
- `divan merge <task-id>`
- `divan cleanup <task-id>`

Kurallar:

- Merge işlemi otomatik yapılmaz; kullanıcı komutu gerektirir.
- Dirty source repo varsa worktree yaratmadan önce CLI uyarır.
- `git worktree add` hataları trace'e yazılır.
- Salt okunur review task'ı yeni worktree yaratmak yerine implement worktree diff'ini okuyabilir.
- `divan cleanup <task-id>` yalnız worktree temizler; artifact'lara dokunmaz.
- Artifact referans sayımı ve `divan gc` v2 kapsamıdır.

### 5.7 Trace Collector

Sorumluluklar:

- Her task için trace üretmek.
- Her agent session için span üretmek.
- Event append işlemini ucuz ve deterministik tutmak.
- CLI/TUI için timeline query sağlamak.

Event isimleri:

```text
task_created
task_transition
router_decision
agent_registered
spawn
session_event
tool_call
file_edit
artifact_published
msg_sent
msg_delivered
policy_denied
worktree_created
worktree_diffed
worktree_merge_requested
worktree_merged
error
```

Faz 4 çıktısı:

- `divan trace <task-id>` timeline.
- Ajan şeritleri, mesaj okları ve state transition listesi.
- HTML export v1.5 kapsamındadır; Faz 4 kabul kriteri değildir.

### 5.8 Daemon yaşam döngüsü ve watchdog

Sorumluluklar:

- Daemon tekilliği için lock file almak: `~/.local/share/divan/divan.lock`.
- İkinci daemon başlatma denemesini mevcut daemon'a yönlendirmek.
- Startup reconciliation yapmak: DB'deki aktif PID/session kayıtlarını gerçek süreçlerle karşılaştırmak.
- Ölü session'a bağlı `claimed|working|review` task'larını `failed` durumuna çekmek.
- Orphan geçişlerini `trace_events` içinde `reason=orphaned` olarak kaydetmek.
- İlgili agent kayıtlarını `offline` yapmak.
- Session watchdog çalıştırmak: `tasks.max_runtime_secs` veya kind-bazlı config varsayılanı aşılırsa session'ı sonlandırmak.
- Timeout geçişlerini `trace_events` içinde `reason=timeout` olarak kaydetmek.

v1 sınırları:

- Daemon ölümünde hayatta kalan ajan süreçlerine re-attach yapılmaz.
- Re-attach v2 adayıdır.
- Watchdog hub'ın kendi yetkisidir; policy kontrolüne takılmaz, ancak trace'e yazılır.

Testler:

- İkinci daemon başlatma denemesi yeni daemon açmaz.
- Sahte PID ile çalışan task startup'ta `failed(orphaned)` olur.
- Takılan fake adapter `max_runtime_secs` aşınca `failed(timeout)` olur.

---

## 6. Adaptör Implementasyon Planı

### 6.1 Ortak adapter trait

`divan-adapters` içinde:

```rust
#[async_trait]
pub trait AgentAdapter {
    fn card(&self) -> AgentCard;
    async fn spawn(&self, task: &Task, ctx: SpawnCtx) -> Result<SessionHandle>;
    async fn deliver(&self, handle: &SessionHandle, batch: MessageBatch) -> Result<DeliveryReceipt>;
    async fn observe(&self, handle: &SessionHandle) -> Result<EventStream>;
    async fn resume(&self, session_id: &str, prompt: &str) -> Result<SessionHandle>;
    async fn kill(&self, handle: &SessionHandle) -> Result<()>;
}
```

Ek tipler:

- `SpawnCtx`: worktree path, env, policy snapshot, artifact refs, trace/span id.
- `SessionHandle`: tool, pid, session id, task id, started_at, delivery support.
- `DeliveryReceipt`: accepted, injected_at, raw status.
- `EventStream`: normalize edilmiş event stream.
- `AgentError`: class (`retryable` veya `fatal`), raw tool output ref, exit status, diagnostic summary.

Hata sınıflandırması:

- `retryable`: rate limit, geçici ağ/araç hatası, geçici MCP bağlantı hatası.
- `fatal`: auth eksik, CLI bulunamadı, parse çöküşü, policy reddi, desteklenmeyen resume.
- Sınıflandıramayan adaptör `fatal` varsayar.
- Scheduler varsayılan olarak `retryable` hataları en fazla 2 kez backoff ile dener; sayı config ile daraltılabilir ama ajan tarafından artırılamaz.

### 6.2 ClaudeAdapter

Faz 0 doğrulama:

- `claude -p --output-format stream-json` parse edilecek.
- Resume için session id yakalama doğrulanacak.
- Hook injection ve Stop hook sinyali test edilecek.

Faz 1 üretim:

- Spawn, observe, resume, kill.
- Stream JSON'dan `ToolCall`, `FileEdit`, `TurnEnd`, `Error{retryable|fatal}` map'i.
- Başarısız parse durumunda raw event artifact/log olarak saklanır.

Faz 2:

- Hook install desteği.
- Turn boundary message batch injection.
- Idle wake.

### 6.3 CodexAdapter

Faz 0 doğrulama:

- `codex e --json` output modeli belgelenecek.
- Resume davranışı doğrulanacak.

Faz 1 üretim:

- Spawn, observe, resume, kill.
- JSON stream event normalizasyonu.
- `Error{retryable|fatal}` taksonomisi.
- Review görevlerinde salt okunur policy ile çalıştırma.

Faz 2:

- Hook install veya MCP ağırlıklı teslim yolu. Tool desteği doğrulamaya göre Card'da beyan edilir.

### 6.4 CopilotAdapter

Faz 0 doğrulama:

- `copilot -p` exit code, stdout/stderr, timeout davranışı.
- `--allow-tool` filtrelerinin gerçek syntax'ı.
- MCP kayıt akışı.

Faz 3 üretim:

- One-shot veya sınırlı multi-turn davranış, doğrulama sonucuna göre.
- `--allow-tool` compiler:
  - `write` capability -> yalnız task worktree path'i
  - shell allowlist -> task policy
  - URL/MCP allowlist -> config
- FileEdit event'leri diff'ten türetilir.
- ToolCall granülaritesi eksikse trace'e `tool_call_unavailable` metadata yazılır.

### 6.5 AgyAdapter

Faz 0 doğrulama:

- `agy -p` output, exit code, timeout.
- Conversation id'nin disk veya log üzerinden bulunup bulunmadığı.
- Issue #7 durumu.

v1 kararı:

- Yalnız one-shot task kind'ları: `review`, `research`, `analyze`, `report`.
- `multi_turn = false`.
- `resume` her zaman `Unsupported`.
- Cost Router multi-turn işlerde exclude eder.

---

## 7. CLI Komut Planı

### Faz 1 komutları

```text
divan up
divan down
divan run "<görev>" --flow write-review --repo <path>
divan status
divan log [--task <id>] [--trace <id>]
divan diff <task-id>
divan merge <task-id>
divan cleanup <task-id>   # yalnız worktree; artifact silmez
```

### Faz 2 komutları

```text
divan install-hooks [--tool claude|codex|all]
divan mcp print-config --tool claude|codex|copilot|agy
divan messages [--agent <id>] [--task <id>]
divan agents
```

### Faz 3 komutları

```text
divan policy show [--agent <id>]
divan policy grant <agent-id> <capability>
divan router explain <task-id>
divan config validate
```

### Faz 4 komutları

```text
divan tui
divan trace <task-id|trace-id>
divan trace export <task-id|trace-id> --format html   # v1.5; v1 kabul kriteri değil
```

---

## 8. Konfigürasyon ve Lokal State

### 8.1 Kullanıcı config

Önerilen dosya:

```text
~/.config/divan/config.toml
```

İçerik:

```toml
[daemon]
socket = "~/.local/share/divan/divan.sock"
db = "~/.local/share/divan/divan.db"

[defaults]
router_rules = "~/.config/divan/router.rules.yaml"
artifact_root = ".divan/artifacts"
worktree_root = ".divan/worktrees"

[runtime_defaults]
implement_max_runtime_secs = 1800
review_max_runtime_secs = 900
test_max_runtime_secs = 1200

[[agents]]
id = "claude-1"
tool = "claude"
cost_class = 5
skills = ["implement", "architecture"]
capabilities = ["read", "write", "delegate"]

[[agents]]
id = "codex-review"
tool = "codex"
cost_class = 4
skills = ["review", "test"]
capabilities = ["read"]
```

### 8.2 Proje state

Her hedef repo içinde:

```text
.divan/
  artifacts/
  worktrees/
  runs/
```

Global daemon state:

```text
~/.local/share/divan/
  divan.db
  divan.sock
  logs/
```

Kural:

- Artifact ve worktree hedef repo ile ilişkili olmalı.
- Registry, trace ve message state global DB'de tutulabilir.
- Path'ler canonicalize edilmeden policy kontrolünden geçmemeli.

---

## 9. Faz Bazlı İş Kırılımı

## Faz 0 - Doğrulama Spike'ları

Süre: 1-2 hafta.

Amaç: CLI araç varsayımlarını throwaway kodla doğrulamak. Üretim kodu yazılmayacak; yalnız rapor ve karar üretilecek.

### F0.1 Spike çalışma alanı

İşler:

- `docs/spikes/` dizini oluştur.
- Her spike için template oluştur:
  - amaç
  - komutlar
  - gözlemler
  - raw örnek çıktı konumu
  - karar
  - üretim etkisi
- Throwaway kodu `spikes/` altında tut; Faz 0 sonunda üretim crate'lerine kopyalama.

Kabul:

- 5 spike raporu aynı formatta.
- Ham output örnekleri kişisel token veya secret içermeden saklanmış.

### F0.2 S1 Claude stream-json

İşler:

- `claude -p --output-format stream-json` ile küçük implement/review prompt'ları çalıştır.
- Event türlerini çıkar.
- Session id ve resume için gereken alanları kaydet.
- Parse hatası durumunda fallback stratejisini yaz.

Kabul:

- `docs/spikes/s1_claude_stream_json.md`
- Normalize event mapping tablosu.
- ClaudeAdapter için minimum spawn/observe/resume gereksinimleri.

### F0.3 S2 Codex json

İşler:

- `codex e --json` ile örnek görev çalıştır.
- Claude output modeliyle farkları belgeleyerek ortak `NormalizedEvent` taslağı çıkar.
- Resume id ve exit code davranışını doğrula.

Kabul:

- `docs/spikes/s2_codex_json.md`
- Ortak event modelinin ilk versiyonu.

### F0.4 S3 Hook injection

İşler:

- Claude Code hook config formatını doğrula.
- Stop/turn-end hook'undan oyuncak hub'a sinyal gönder.
- Hook ile context sonuna küçük mesaj bloğu enjekte etmeyi kanıtla.
- Divan kapalıyken no-op davranışı doğrula.

Kabul:

- `docs/spikes/s3_hook_injection.md`
- Hook install/uninstall risk notları.

### F0.5 S4 rmcp oyuncak server

İşler:

- 2 tool'luk minimal MCP server yaz:
  - `send_message`
  - `publish_artifact`
- Claude Code'a kaydet.
- Tool çağrısı gidiş-dönüşünü doğrula.
- Tool schema token maliyeti için kısa açıklama yaz.

Kabul:

- `docs/spikes/s4_rmcp_toy_server.md`
- Divan MCP tool setinin minimum şema kararı.

### F0.6 S5 Copilot ve Agy davranışı

İşler:

- `copilot -p` stdout/stderr/exit/timeout davranışını ölç.
- Copilot `--allow-tool` desenlerini deneyip policy compiler notu çıkar.
- `agy -p` stdout/stderr/exit/timeout davranışını ölç.
- Agy conversation id'nin dosyadan bulunup bulunmadığını araştır.
- Agy issue #7 durumunu rapora yaz.

Kabul:

- `docs/spikes/s5_copilot_agy.md`
- 3.3 uyumluluk matrisi için güncelleme önerisi.

### F0.7 Dil kararı ve ADR

İşler:

- Rust ilerleme hızını, crate olgunluğunu ve tool uyumunu değerlendir.
- Rust devam veya C# B planına geçiş kararını ver.
- `docs/adr/0001-language-and-runtime.md` yaz.

Kabul:

- Dil kararı kilitli.
- Faz 1 scope'u bu karara göre başlatılabilir.

---

## Faz 1 - MVP: Yaz, Review, Rapor

Süre: 3-6. hafta.

Amaç: Gerçek repoda tek komutla Claude implement, Codex review, artifact raporu akışını çalıştırmak.

### F1.1 Workspace bootstrap

İşler:

- Cargo workspace oluştur.
- Temel crate'leri aç: `core`, `db`, `daemon`, `cli`, `adapters`, `trace`.
- Ortak error tipi ve logging/tracing setup.
- `cargo fmt`, `cargo clippy`, `cargo test` temel CI komutları.

Kabul:

- `cargo test --workspace` boş iskelette geçer.
- CLI `divan --version` çalışır.

### F1.2 SQLite migration ve store

İşler:

- `migrations/0001_initial.sql` yaz.
- `tasks.max_runtime_secs` ve `messages.origin_message_id` alanlarını ekle.
- `message_deliveries` tablosunu ekleme; v1 fan-out modeli hedef başına message row kullanır.
- WAL mode enable.
- Store trait/repository sınırları:
  - `AgentStore`
  - `TaskStore`
  - `MessageStore`
  - `ArtifactStore`
  - `TraceStore`
- Transaction helper.

Kabul:

- In-memory/temp DB testleri.
- State transition ve trace event aynı transaction içinde doğrulanır.
- Fan-out kopyası `origin_message_id` ile kaynak yayına bağlanır.
- `max_runtime_secs = NULL` olduğunda kind-bazlı varsayılan okunabilir.

### F1.3 Daemon ve JSON-RPC IPC

İşler:

- Unix socket server.
- JSON-RPC request/response envelope.
- `ping`, `status`, `shutdown`.
- CLI daemon discovery.
- Lock file ile tek daemon garantisi.
- Startup reconciliation: ölü session task'larını `failed(orphaned)` yapma.
- Session watchdog: `max_runtime_secs` aşımında kill + `failed(timeout)`.
- Stale socket cleanup.

Kabul:

- `divan up` daemon başlatır.
- İkinci `divan up` yeni daemon açmadan mevcut daemon'a bağlanır.
- `divan status` daemon'dan cevap alır.
- `divan down` temiz kapanır.
- Sahte aktif PID startup'ta `failed(orphaned)` olur.
- Takılan fake session watchdog ile `failed(timeout)` olur.

### F1.4 Artifact Store

İşler:

- Content-addressed write/read.
- Summary length validation.
- MIME ve byte metadata.
- CLI üzerinden debug okuma.

Kabul:

- Aynı içerik aynı ref'i üretir.
- 400 karakter üstü summary reddedilir.
- Artifact path traversal mümkün değildir.

### F1.5 Task Scheduler v1

İşler:

- Task create/claim/transition.
- Lineer dependency çözümü.
- `write-review` flow state machine.
- Trace event wiring.
- `Error{retryable|fatal}` sınıfına göre deterministik retry.

Kabul:

- Fake adapter ile implement -> review -> done akışı test edilir.
- Failed implement durumunda review başlamaz.
- `retryable` hata sınırlı backoff ile yeniden denenir.
- `fatal` hata doğrudan `failed` geçişi üretir.

### F1.6 Worktree Manager v1

İşler:

- Hedef repo doğrulama.
- `git worktree add` wrapper.
- Branch/path naming.
- Diff artifact üretimi.
- `divan diff`, `divan merge` ve `divan cleanup`.

Kabul:

- Temp git repo testinde worktree yaratılır.
- Diff artifact üretimi deterministic.
- Merge kullanıcı komutu olmadan yapılmaz.
- `divan cleanup <task-id>` worktree'yi temizler ama artifact'lara dokunmaz.

### F1.7 ClaudeAdapter üretim minimumu

İşler:

- Faz 0 event mapping'inden parser.
- Spawn process yönetimi.
- Worktree cwd ile çalıştırma.
- Stream observe.
- Kill/timeout.
- `Error{retryable|fatal}` mapping.

Kabul:

- Fake fixture stream parse testleri.
- Gerçek CLI testi feature/env gate arkasında: `DIVAN_TEST_CLAUDE=1`.

### F1.8 CodexAdapter üretim minimumu

İşler:

- Faz 0 event mapping'inden parser.
- Spawn process yönetimi.
- Review prompt packaging.
- Stream observe.
- `Error{retryable|fatal}` mapping.

Kabul:

- Fake fixture stream parse testleri.
- Gerçek CLI testi feature/env gate arkasında: `DIVAN_TEST_CODEX=1`.

### F1.9 CLI write-review

İşler:

- `divan run "<görev>" --flow write-review --repo <path>`.
- Spec artifact oluşturma.
- Flow progress output.
- Failure durumunda trace/log yönlendirme.

Kabul:

- Temp repo + fake adapter ile e2e test.
- Gerçek repo demo komutu dokümante edilir.
- Çıktıda final report artifact ref görünür.

### F1.10 Faz 1 demo

İşler:

- Hedef gerçek repoda demo çalıştır.
- Demo GIF kaydet.
- Known limitations belgesi yaz.

Kabul:

- Implement diff artifact.
- Review artifact.
- Final report manifest.
- `divan log` ile tüm state geçişleri görünür.
- Daemon yeniden başlatıldığında öksüz task'lar `failed(orphaned)` olur.
- Takılı sahte adaptör watchdog ile `failed(timeout)` olur.

---

## Faz 2 - Mesajlaşma Çekirdeği

Süre: 7-9. hafta.

Amaç: Ajanlar arası mesajlaşmayı hook + MCP ile token-bilinçli ve push tabanlı hale getirmek.

### F2.1 Hook installer

İşler:

- `divan install-hooks`.
- Claude hook dosyaları.
- Codex hook desteği Faz 0 sonucuna göre.
- No-op davranışı: daemon yoksa sessiz çık.
- Mevcut kullanıcı hook config'i ile merge.
- Kurulum öncesi backup.
- Uninstall stratejisi.

Kabul:

- Hook install idempotent.
- Mevcut kullanıcı config'i yedeklenir.
- Mevcut kullanıcı hook girdileri ezilmez.
- Daemon kapalıyken CLI aracı bozulmaz.

### F2.2 Activity ve turn boundary RPC

İşler:

- Hook -> daemon RPC:
  - `activity.report`
  - `turn.end`
  - `session.idle`
- Daemon pending batch üretir.
- Hook batch'i context sonuna ekler.
- Batch'in ilk satırına sabit davranış yönergesi eklenir.

Kabul:

- İki bekleyen mesaj tek injection bloğunda teslim edilir.
- Injection trace event üretir.
- Enjekte edilen blok, ajanın serbest metinle cevap vermemesi ve yalnız `send_message` MCP tool'u kullanması gerektiğini belirtir.

### F2.3 MCP server

İşler:

- `send_message`.
- `delegate_task`.
- `claim_task`.
- `complete_task`.
- `publish_artifact`.
- `get_artifact`.
- `subscribe`.
- `list_agents`.
- Tüm tool handlers policy kontrolünden geçer.

Kabul:

- Oyuncak MCP client ile tool tests.
- Claude/Codex kayıt talimatı dokümante edilir.
- Tool açıklamaları kısa ve token-bilinçli.

### F2.4 Subscription engine

İşler:

- Event filter JSON modeli.
- Agent subscription kayıtları.
- Fan-out resolution.
- Direkt mesaj ve subscription mesaj ayrımı.
- Kaynak yayın satırı + hedef başına kopya modeli.
- `origin_message_id` ilişkisinin trace edge olarak kullanımı.

Kabul:

- Abone olmayan ajan mesaj almaz.
- Broadcast capability olmadan topic mesajı reddedilir.
- Fan-out teslimleri trace'de izlenir.
- Teslim kuyruğundaki tüm fan-out satırlarında `to_agent` doludur.
- Kopyalar aynı `origin_message_id` ile kaynak yayına bağlanır.

### F2.5 Batching ve delivery receipts

İşler:

- Agent bazlı pending queue.
- Turn boundary batch format.
- Delivery receipt.
- Retry/backoff policy.
- `delivered_at` hedefe özel message row üzerinde receipt sonrası yazılır.

Kabul:

- Aynı turn'deki mesajlar tek blok.
- Başarısız teslimde mesaj pending kalır.
- Başarılı teslimde delivered timestamp/receipt yazılır.
- Kaynak yayın satırı teslim kuyruğu gibi işlenmez.

### F2.6 Conflict detection

İşler:

- `file_touches` event yazımı.
- 30 sn pencere içinde farklı agent aynı path'e dokunursa alert.
- Alert subscription ile ilgili ajanlara gider.

Kabul:

- Fake event testinde conflict üretilir.
- Conflict mesajı pointer + kısa summary taşır.

### F2.7 Faz 2 demo

Senaryo:

- Claude implement sırasında Codex'e soru sorar.
- Codex MCP ile cevap artifact publish eder.
- Claude cevabı hook injection ile alır.
- Tüm trafik `divan log` ve trace'de görünür.
- Fan-out varsa kaynak yayın ile hedef kopyaları trace'te ilişkilidir.

Kabul:

- Mesaj summary ortalaması raporlanır.
- Polling yoktur; ajan manuel "mesajım var mı" tool'u çağırmaz.
- Injection bloğundaki yönerge demo çıktısında görülür.

---

## Faz 3 - İzin, Maliyet, 3. Adaptör

Süre: 10-12. hafta.

Amaç: Divan'ın farklılaştırıcı iki katmanı olan policy ve cost router'ı üretim davranışına bağlamak; Copilot'u üçüncü adaptör olarak eklemek.

### F3.1 Policy Engine core

İşler:

- Action modeli.
- Capability validation.
- Path canonicalization.
- Worktree boundary check.
- Policy denial trace.

Kabul:

- Yetkisiz write reddedilir.
- Yetkisiz kill reddedilir.
- Her red trace'de reason ile görünür.

### F3.2 Hook PreToolUse path kontrolü

İşler:

- Destekleyen araçlarda write path kontrolü.
- Worktree dışı path için reject mesajı.
- Reject kısa ve eyleme yönelik.

Kabul:

- Worktree dışına yazma denemesi engellenir.
- Agent session kırılmadan devam eder.

### F3.3 Router YAML engine

İşler:

- YAML parse.
- Match/prefer/exclude modeli.
- `different_vendor_than`.
- `multi_turn` exclude.
- `router explain`.

Kabul:

- Review task writer'dan farklı vendor'a atanır.
- Multi-turn task agy'ye atanmaz.
- Karar trace'de rule id ile görünür.

### F3.4 CopilotAdapter

İşler:

- Spawn.
- Output capture.
- Diff tabanlı FileEdit event.
- Policy -> `--allow-tool` compile.
- MCP config desteği.

Kabul:

- Copilot'a verilen yazma görevi yalnız worktree path'ine izinli.
- `--allow-tool` argümanları `router/policy explain` çıktısında görünür.
- ToolCall granülarite eksikliği dokümante edilir.

### F3.5 AgyAdapter degraded

İşler:

- One-shot spawn.
- `multi_turn = false`.
- Unsupported resume.
- Router exclude rule.

Kabul:

- `review` veya `research` one-shot task çalışır.
- Multi-turn task agy'ye seçilmez.

### F3.6 Faz 3 demo

Senaryo:

- Writer Claude veya Copilot.
- Reviewer farklı vendor.
- Yetkisiz kill denemesi policy_denied.
- Copilot write izni worktree path'e daraltılmış.

Kabul:

- Policy denial trace'de görünür.
- Router karar açıklaması CLI'da görünür.
- Copilot üçüncü adaptör olarak AgentCard listesinde.

---

## Faz 4 - Gözlemlenebilirlik ve TUI

Süre: 13-15. hafta.

Amaç: 3 ajanlı akışı tek ekrandan izlenebilir hale getirmek.

### F4.1 Trace query API

İşler:

- Trace timeline query.
- Span grouping.
- Task tree query.
- Message edge query.
- Metrics aggregate:
  - mesaj başına ortalama summary uzunluğu
  - oturum başına injection sayısı
  - görev başına cost_class dağılımı

Kabul:

- `divan trace <id>` text output üretir.
- Eksik event timeline'ı kırmaz.

### F4.2 Ratatui ana ekran

Paneller:

- Agents: status, tool, cost_class, current task.
- Tasks: state, assignee, deps.
- Messages: live flow.
- Trace/detail: seçili task event'leri.

Kabul:

- 3 ajanlı demo canlı izlenir.
- Keyboard navigation minimum: yukarı/aşağı, enter detay, q çıkış.

### F4.3 Token/maliyet metrikleri

İşler:

- Summary length ölçümü.
- Injection count.
- Cost class distribution.
- "Yüzde tasarruf" iddiası olmadan raporlama.

Kabul:

- TUI veya CLI metrikleri gösterir.
- README için gerçek demo sayıları çıkarılabilir.

### F4.4 HTML export sınırı

İşler:

- Statik HTML export v1.5 kapsamındadır.
- Faz 4'te yalnız açıkça planlanmış opsiyonel feature flag denemesi yapılabilir.
- v1 acceptance için `divan trace <id>` text/TUI timeline yeterlidir.

Kabul:

- HTML export Faz 4 kabulünü bloklamaz.
- README roadmap içinde v1.5 maddesi olarak görünür.

---

## Faz 5 - Cila ve Lansman

Süre: 16-18. hafta.

Amaç: Dış kullanıcının 10 dakikada kurup MVP akışını çalıştırabileceği paket ve dokümantasyon.

### F5.1 CI

İşler:

- GitHub Actions:
  - fmt
  - clippy
  - test
  - build macOS/Linux
- Adapter contract testleri mock/fixture ile.
- Gerçek CLI smoke testleri env-gated.

Kabul:

- PR check'leri stabil.
- External CLI yoksa CI fail olmaz.

### F5.2 Paketleme

İşler:

- `cargo install` yolu.
- Shell installer.
- Hook installer docs.
- Uninstall docs.

Kabul:

- Temiz makinede kurulum adımları 10 dakikadan kısa.
- Installer mevcut hook config'lerini ezmez.

### F5.3 Dokümantasyon

İşler:

- İngilizce README:
  - problem
  - K1-K10 özeti
  - mimari diyagram
  - install
  - demo GIF
  - limitations
- ADR dosyaları:
  - language/runtime
  - deterministic hub
  - pointer messages
  - worktree isolation
  - policy model
- CONTRIBUTING.

Kabul:

- Yeni kullanıcı "write-review" akışını docs ile çalıştırabilir.
- Kapsam dışı maddeler açıkça yazılı.

### F5.4 Lansman hazırlığı

İşler:

- Demo repo senaryosu.
- Show HN metni.
- r/ClaudeCode paylaşım metni.
- awesome-agent-orchestrators PR hazırlığı.
- LinkedIn yazı başlıkları.

Kabul:

- Dış kullanıcıdan ilk issue/star hedeflenebilir durumda.

---

## 10. Test Stratejisi

### 10.1 Unit testler

Zorunlu alanlar:

- Task state machine.
- `Error{retryable|fatal}` retry kararı.
- Summary length validation.
- Artifact hash/dedupe.
- Policy matrix.
- Path canonicalization.
- Router rule scoring.
- Subscription matching.
- Fan-out `origin_message_id` bağlama.
- Event parser fixture'ları.

### 10.2 Integration testler

Fake adapter ile:

- write-review akışı.
- failed implement -> review başlamaz.
- message batching.
- fan-out source/copy delivery.
- policy denial.
- daemon startup reconciliation.
- watchdog timeout.
- worktree diff artifact.

Temp git repo ile:

- worktree create/diff/cleanup.
- merge command.
- write boundary.
- cleanup artifact'lara dokunmama.

### 10.3 Contract testler

Gerçek CLI araçları opsiyonel environment flag ile:

```text
DIVAN_TEST_CLAUDE=1
DIVAN_TEST_CODEX=1
DIVAN_TEST_COPILOT=1
DIVAN_TEST_AGY=1
```

Kurallar:

- CI'da default kapalı.
- Yerel smoke test dokümante.
- Raw çıktılar secret scan'den geçirilmeden commitlenmez.

### 10.4 E2E demo testleri

Senaryolar:

1. Tek repo, `write-review`.
2. MCP soru-cevap + hook delivery.
3. Policy denied.
4. Router different vendor.
5. Copilot allow-tool path sınırı.
6. Daemon restart -> orphaned task reconciliation.
7. Watchdog timeout.

---

## 11. İlk 10 Çalışma Günü İçin Somut Plan

### Gün 1

- `docs/spikes/` ve spike template.
- Tool availability check script.
- Claude S1 komut denemeleri.

### Gün 2

- Claude stream parser spike.
- S1 raporu.
- Codex S2 komut denemeleri.

### Gün 3

- Codex json parser spike.
- Ortak `NormalizedEvent` taslağı.
- S2 raporu.

### Gün 4

- Claude hook injection denemesi.
- Stop/turn-end sinyali.
- S3 raporu.

### Gün 5

- `rmcp` toy MCP server.
- Claude MCP roundtrip.
- S4 raporu.

### Gün 6

- Copilot programatik mod denemeleri.
- `--allow-tool` policy mapping notları.

### Gün 7

- Agy programatik mod denemeleri.
- Conversation id araştırması.
- S5 raporu.

### Gün 8

- Rust/C# değerlendirmesi.
- ADR 0001.
- 3.3 uyumluluk matrisi güncelleme önerisi.

### Gün 9

- Cargo workspace bootstrap.
- Core domain tipleri.
- İlk migration taslağı.

### Gün 10

- Daemon/CLI ping iskeleti.
- `cargo test --workspace` ve formatting.
- Faz 1 backlog refine.

---

## 12. Definition of Done

Bir iş paketinin tamamlanmış sayılması için:

- K1-K10 ile çelişmiyor.
- Unit veya integration test mevcut; test eklenemiyorsa nedeni not edilmiş.
- Trace event üretmesi gereken davranış trace'e yazılıyor.
- Ağır içerik message payload'a değil artifact'a gidiyor.
- Policy-relevant action policy engine'den geçiyor.
- CLI hataları kullanıcıya kısa ve eyleme dönük dönüyor.
- Docs veya ADR gerekiyorsa güncellenmiş.

Bir fazın tamamlanmış sayılması için:

- Faz kabul kriterleri canlı veya kaydedilmiş demo ile gösterildi.
- Known limitations yazıldı.
- Bir sonraki faza devredilen riskler açık.
- Kapsam dışı fikirler roadmap'e alındı, koda eklenmedi.

---

## 13. Risklere Göre Teknik Önlemler

| Risk | Önlem | İlgili faz |
| --- | --- | --- |
| CLI output format değişimi | Fixture parser testleri + adapter izolasyonu | F0-F1 |
| Hook API değişimi | Hook logic minimal, hub RPC ağırlıklı | F2 |
| rusqlite async bloklama | SQLite actor veya `spawn_blocking` sınırı | F1 |
| Message delivery çok alıcı karmaşıklığı | v1 Seçenek A: hedef başına message row + `origin_message_id` | F2 |
| Daemon çökmesi aktif oturumları öksüz bırakır | Lock file + startup reconciliation + `failed(orphaned)` | F1 |
| Takılan ajan sistemi süresiz kilitler | `max_runtime_secs` + watchdog + `failed(timeout)` | F1 |
| Worktree path escape | Canonical path + policy tests | F1-F3 |
| Kapsam şişmesi | Faz acceptance gate + ADR/backlog disiplini | Tüm fazlar |
| Gerçek CLI testlerinin CI'da kırılganlığı | Env-gated contract tests, fixture default | Tüm fazlar |
| Token tasarrufu iddiasının ölçülememesi | Sadece ölçülen proxy metriklerini raporla | F4-F5 |

---

## 14. Öncelikli Backlog Özeti

P0:

- Faz 0 spike raporları.
- Dil kararı ADR.
- Cargo workspace.
- SQLite schema/store.
- Daemon IPC.
- Daemon lock/reconciliation/watchdog.
- Artifact Store.
- Task Scheduler.
- Claude/Codex adapter minimumu.
- `Error{retryable|fatal}` adapter mapping.
- Worktree Manager.
- `write-review` CLI.

P1:

- Hook installer.
- MCP server.
- Message bus batching.
- Subscription engine.
- Fan-out `origin_message_id` modeli.
- Hook merge/yedek + injection yönergesi.
- Policy Engine.
- Router YAML.
- CopilotAdapter.

P2:

- Agy degraded adapter.
- TUI.
- Trace timeline.
- Token/cost metrics.

P3:

- Packaging.
- README/GIF.
- CI hardening.
- Launch material.
- v1.5 HTML trace export.

---

## 15. Bir Sonraki En İyi Adım

Bu plan onaylandıktan sonra ilk gerçek çalışma Faz 0 ile başlamalı:

1. `docs/spikes/` şablonları oluşturulur.
2. S1-S5 doğrulama komutları çalıştırılır.
3. Sonuçlara göre AgentAdapter event modeli ve 3.3 uyumluluk matrisi güncellenir.
4. Rust kararı ADR ile kilitlenir.

Faz 0 bitmeden üretim hub koduna başlanmamalıdır; aksi halde adapter varsayımları yanlış çıkarsa Faz 1 iskeleti gereksiz yeniden yazım riski taşır.
