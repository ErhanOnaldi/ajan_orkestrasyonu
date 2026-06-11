#!/usr/bin/env bash
# Divan Faz 0 — Araç uygunluk kontrolü (implementation_plan.md Gün 1)
# Throwaway spike yardımcı script'i. Üretim koduna kopyalanmaz.
# Hiçbir araç çağırmaz, yalnız varlık + sürüm raporlar. Secret yazmaz.
set -u

echo "== Divan Faz 0 araç uygunluk kontrolü =="
echo "tarih: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "platform: $(uname -s) $(uname -m)"
echo

check() { # ad, komut, sürüm-bayrağı
  local name="$1" cmd="$2" verflag="${3:-}"
  if command -v "$cmd" >/dev/null 2>&1; then
    local path ver=""
    path="$(command -v "$cmd")"
    if [ -n "$verflag" ]; then ver="$("$cmd" "$verflag" 2>&1 | head -1)"; fi
    printf "  %-12s ✅  %s  %s\n" "$name" "$path" "$ver"
  else
    printf "  %-12s ⛔  KURULU DEĞİL\n" "$name"
  fi
}

echo "[ Çekirdek / toolchain ]"
check rust    cargo  --version
check git     git    --version
check python3 python3 --version
check jq      jq     --version

echo
echo "[ Faz 0 hedef ajan CLI'ları (spec §3.3) ]"
check claude   claude   --version   # S1, S3, S4 — referans adaptör
check codex    codex    --version   # S2
check copilot  copilot  --version   # S5
check agy      agy      --version   # S5 (degraded)
check opencode opencode --version   # TAM (doğrulanacak)

echo
echo "Not: ⛔ işaretli araçlar için ilgili spike bloke olur (rapor: durum=⛔)."
