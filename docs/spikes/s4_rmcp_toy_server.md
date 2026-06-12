# Spike S4 — `rmcp` ile toy stdio MCP server + claude tool-call roundtrip

> Faz 0 doğrulama spike'ı. **Kod/çıktı throwaway'dir; üretim crate'lerine kopyalanmaz.**
> Proje `/tmp/divan_s4/toy-mcp` altında durur, repo dışındadır.
> Spec: `general_plan_and_architecture.md` §3.6-B (MCP face / araç seti) · `implementation_plan.md` F0.5

| Alan | Değer |
| --- | --- |
| Spike | S4 |
| Hedef araç | claude (MCP istemci olarak) + `rmcp` (MCP sunucu) |
| Araç sürümü | `2.1.173 (Claude Code)` · `rmcp 1.7.0` · `cargo 1.92.0` |
| Tarih | 2026-06-11 |
| Durum | ✅ tamamlandı (roundtrip doğrulandı) |

---

## 1. Amaç

Riskli varsayım: **Divan'ın MCP yüzünü (§3.6-B) Rust resmi SDK'sı `rmcp` ile pratik
biçimde kurabilir miyiz, ve gerçek `claude` CLI bu sunucuyu keşfedip araçları
çağırıp dönen JSON'u tüketebilir mi?** Bu, Faz 2 `divan-mcp` crate'inin temel
teknoloji seçimini doğrular:

- `rmcp` sürüm kararlılığı (crate sürümler arası çok değişti; derlenen bir sürüm gerek).
- `#[tool_router]` / `#[tool_handler]` makro yolunun çalışırlığı.
- Araç şemalarının ne kadar küçük/token-ucuz olduğu (tanımlar her turda context'te durur → K2/K4).
- `summary <= 400` mesaj kuralının (CLAUDE.md "Message Rules") sunucu tarafında uygulanabilirliği.

İki araç, Divan'ın gerçek MCP yüzünün küçük bir aynası olarak seçildi:
`send_message` ve `publish_artifact`.

## 2. Komutlar

```bash
# 1) Derle (rmcp 1.7.0, stdio transport)
cd /tmp/divan_s4/toy-mcp && cargo build 2>&1 | tail -5

# 2) MCP handshake smoke test (initialize)
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  | /tmp/divan_s4/toy-mcp/target/debug/toy_mcp

# 3) İzole MCP config (kullanıcının global connector'larına dokunmaz)
cat > /tmp/divan_s4/mcp.json <<EOF
{ "mcpServers": { "divan": { "command": "/tmp/divan_s4/toy-mcp/target/debug/toy_mcp", "args": [] } } }
EOF

# 4) Gerçek roundtrip — claude'u araçlara yönlendir, diğer MCP'lerden izole et
claude -p "Call the send_message tool with to='codex-rev', kind='status', summary='spike S4 roundtrip ok'. Then call publish_artifact with content='hello divan'. Report the returned refs." \
  --mcp-config /tmp/divan_s4/mcp.json --strict-mcp-config \
  --allowedTools "mcp__divan__send_message" "mcp__divan__publish_artifact" \
  --output-format stream-json --verbose --max-turns 6 < /dev/null \
  > /tmp/divan_s4/run.jsonl 2>/tmp/divan_s4/run.err

# 5) Roundtrip'i kanıtla (jq)
jq -c 'select(.type=="assistant") | .message.content[] | select(.type=="tool_use") | {name,input}' run.jsonl
jq -c 'select(.type=="user") | .message.content[] | select(.type=="tool_result") | {is_error,content}' run.jsonl

# 6) Araç şemalarını sunucudan oku (claude'un her turda taşıdığı bytes)
{ printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}';
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}';
  printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'; sleep 0.3; } \
  | /tmp/divan_s4/toy-mcp/target/debug/toy_mcp | grep '"id":2' | jq '.result.tools'
```

Notlar / gotcha'lar:
- `claude -p` stdin bağlıyken ~3 sn bloklar; **her zaman `< /dev/null`** eklendi.
- `--strict-mcp-config` kullanıcının claude.ai connector'larından izole eder; `tools` listesi küçük kalır.
- `claude mcp get` bu bayrakları (`--mcp-config`, `--strict-mcp-config`) kabul etmiyor →
  şemalar yetkili kaynaktan, yani **sunucunun `tools/list` cevabından** alındı.
- `--allowedTools` adlandırması doğru çıktı: `mcp__<server>__<tool>` → `mcp__divan__send_message`.

## 3. Gözlemler

### 3.1 rmcp 1.7.0 derleyen minimal `Cargo.toml`

```toml
[dependencies]
rmcp = { version = "1.7.0", features = ["server", "macros", "transport-io"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "io-std"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1"
blake3 = "1"
```

API tuzakları (WebFetch özetleri yanıltıcıydı, gerçek 1.7.0 kaynağından doğrulandı):

| Beklenen | Gerçek (1.7.0) |
| --- | --- |
| `rmcp::handler::server::tool::Parameters` | `Parameters` aslında `handler::server::wrapper::Parameters` |
| `ServerInfo { .. }` struct literal | `ServerInfo`/`Implementation` `#[non_exhaustive]` → literal yasak; `ServerInfo::new(caps).with_server_info(..).with_instructions(..)` builder zinciri kullanıldı |
| `async fn get_info` + `async_trait` | `get_info` **senkron**; makro `#[tool_handler]` `list_tools`/`call_tool`'u kendisi bağlar |
| `serve_server(server, transport)` | İdiyom: `Service::serve(self, stdio()).await?` → `RunningService`, sonra `.waiting().await?` |

Çalışma deseni: `struct DivanFace { tool_router: ToolRouter<Self> }`, `new()` içinde
`Self::tool_router()` (makro üretir), `#[tool_router] impl` araçları taşır,
`#[tool_handler] impl ServerHandler` yüzeyi bağlar. Hatasız derlendi (yalnız
`tool_router never read` false-positive uyarısı — alanı makro okuyor).

### 3.2 MCP handshake çalışıyor

`initialize` cevabı sunucudan döndü: `protocolVersion 2024-11-05`,
`capabilities.tools{}`, `serverInfo.name = "divan-toy-mcp"`.

### 3.3 Roundtrip — KANITLANDI

`claude` 5 turda bitti (`result.subtype=success`, `is_error=false`, ~19 s).
`tool_use` (assistant) → `tool_result` (user) eşleşmesi:

| tool_use `name` | input | dönen `tool_result` (sunucu JSON'u) |
| --- | --- | --- |
| `mcp__divan__send_message` | `{to:"codex-rev", kind:"status", summary:"spike S4 roundtrip ok"}` | `{"status":"queued","summary_len":21}` |
| `mcp__divan__publish_artifact` | `{content:"hello divan"}` | `{"bytes":11,"ref":"8fa2d536…9d11"}` |

(İlk iki tur claude'un kendi `ToolSearch`'ü idi — MCP araçları deferred geldiği için
önce şemalarını yükledi; bu Divan ile ilgisiz, claude harness davranışı.)

`publish_artifact` ref'i **bağımsız doğrulandı**: ayrı bir `blake3` crate programı
`"hello divan"` için aynı hash'i üretti —
`8fa2d5367fa41836a09617583aca8da42c590c281f8e82a8d1f2807ce7019d11`. Eşleşme: evet.

### 3.4 `summary <= 400` kuralı

401 karakterlik `summary` ile ham `tools/call` → sunucu JSON-RPC hata cevabı döndürdü:
`code -32602, "summary too long: 401 chars (max 400)"`. `ErrorData::invalid_params`
ile üretildi. Mesaj kuralı sunucu tarafında uygulanabiliyor.

### 3.5 Minimum şema kararı (token maliyeti)

İki aracın tüm şema dizisi (`tools/list` JSON) **1470 bytes** (~400 token civarı,
pretty-print; tel üzerinde daha küçük). `schemars` üretimi temiz: `send_message`
şeması 4 alan (`to/kind/summary` zorunlu, `artifact_ref` opsiyonel `["string","null"]`),
`publish_artifact` tek alan (`content`). Gereksiz `$defs`/iç içe tip yok.

claude'un gördüğü şema (örnek, `publish_artifact`):

```json
{
  "name": "publish_artifact",
  "description": "Store content and return its blake3 ref.",
  "inputSchema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": { "content": { "type": "string", "description": "Raw content to store; server returns its blake3 ref." } },
    "required": ["content"], "title": "PublishArtifactArgs"
  }
}
```

Çıkarım: alan başına `description` faydalı ama token yiyor; 8 araçlık tam set
(§3.6-B) için açıklamalar **kısa** tutulmalı, `$schema`/`title` gibi alanların
context maliyeti hesaba katılmalı (K2/K4). Düz, tek-seviye şemalar token-ucuz.

## 4. Ham çıktı konumu

`docs/spikes/raw/s4_rmcp_toy_server.txt` — Cargo.toml, kilitli rmcp sürümü,
initialize cevabı, `tools/list` şemaları, roundtrip `tool_use`/`tool_result`
blokları, result event, blake3 cross-check, >400 hata cevabı.
(Sanitize: secret/token yok. claude `session_id` ephemeral lokal UUID'di, dahil edilmedi.)

## 5. Karar

**`rmcp` = Faz 2 `divan-mcp` crate'i için onaylanan MCP sunucu kütüphanesi.**
Sürüm tabanı **1.7.0** (1.x serisi artık kararlı; 0.x makro kaosu geride kaldı).
Resmi makro yolu (`#[tool_router]` + `#[tool_handler]` + `Parameters<T>` +
`schemars`) çalışıyor; el-yazımı `ServerHandler` fallback'ine gerek kalmadı.
stdio transport `claude --mcp-config` ile uçtan uca doğrulandı; araç adlandırması
`mcp__<server>__<tool>` deterministik.

## 6. Üretim etkisi (Faz 2 `divan-mcp`)

Kod değil, gereksinim listesi:

- **SDK**: `rmcp = { version = "1", features = ["server","macros","transport-io"] }`.
  `Parameters` `handler::server::wrapper`'dan; `ServerInfo`/`Implementation`
  `#[non_exhaustive]` → builder zinciri (`ServerInfo::new(..).with_*`) kullan,
  struct literal kullanma.
  > **DÜZELTME (Faz 2 `divan-mcp` implementasyonu, 2026-06-12):** `serverInfo`'yu
  > `Implementation::from_build_env()` ile kurMA — o makro `env!`'leri **rmcp
  > crate'inin** içinde genişler, sonuç `{"name":"rmcp","version":"1.7.0"}` olur.
  > Tüketici crate'te `Implementation::new(env!("CARGO_PKG_NAME"),
  > env!("CARGO_PKG_VERSION"))` kullan → doğru `{"name":"divan-mcp",...}`.
- **Desen**: tek `struct DivanMcp { tool_router, <hub handle/kanallar> }`,
  `#[tool_router] impl` içinde §3.6-B'nin **8 aracı**; `#[tool_handler] impl ServerHandler`.
- **Policy-check wrapping**: her `#[tool]` gövdesi, iş yapmadan **önce** Divan
  policy/state-machine kontrolünü çağırmalı (örn. faz/izin/route kararı). Reddedilen
  çağrı `ErrorData::invalid_params`/uygun MCP hata kodu döndürür — S4'teki
  `summary>400` reddi bu sarmalamanın referans örneğidir.
- **Mesaj kuralı**: `send_message.summary` `<= 400` doğrulaması sunucu kenarında
  zorlanır (CLAUDE.md "Message Rules"); büyük içerik `publish_artifact` →
  artifact ref (blake3) ile taşınır, mesaj yalnız pointer taşır.
- **Artifact store**: `blake3::hash` ref'i içerik-adresli depolamanın anahtarı;
  `{ref, bytes}` cevabı yeterli minimum sözleşme.
- **Şema bütçesi (K2/K4)**: 8 araç tanımı her turda context'te → araç `description`'ları
  ve alan açıklamaları kısa tutulmalı; düz tek-seviye `schemars` şemaları tercih edilir.
  S4'te 2 araç ≈ 1470 bytes ölçüldü; 8 araç için ~4–6 KB beklenir, izlenmeli.
- **Transport**: Faz 2 başlangıcı için stdio yeterli; uzak/çoklu istemci gerekirse
  `rmcp`'nin diğer transport'ları (SSE/streamable-http) ayrı spike ister.
