# Downloads the prebuilt engines (whisper.cpp, llama.cpp) and models Utter bundles.
# Everything lands in src-tauri/resources, which is git-ignored.
# Usage: powershell -ExecutionPolicy Bypass -File scripts/fetch-deps.ps1
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$WhisperTag = 'b5907'
$LlamaTag   = 'b12606'

$root  = Split-Path -Parent $PSScriptRoot
$res   = Join-Path $root 'src-tauri\resources'
$dl    = Join-Path $root '.downloads'
$bin   = Join-Path $res 'bin'
$wbin  = Join-Path $bin 'whisper'
$lbin  = Join-Path $bin 'llama'
$mdir  = Join-Path $res 'models'
New-Item -ItemType Directory -Force $dl, $wbin, $lbin, $mdir | Out-Null

function Get-File($url, $out) {
    if (Test-Path $out) { Write-Host "skip  $(Split-Path -Leaf $out)"; return }
    Write-Host "fetch $url"
    Invoke-WebRequest -Uri $url -OutFile "$out.part" -UseBasicParsing
    Move-Item "$out.part" $out
}

# --- whisper.cpp (CPU, x64) ---
$wz = Join-Path $dl "whisper-$WhisperTag.zip"
Get-File "https://github.com/ggml-org/whisper.cpp/releases/download/$WhisperTag/whisper-bin-x64.zip" $wz
$wx = Join-Path $dl "whisper-$WhisperTag"
if (-not (Test-Path $wx)) { Expand-Archive $wz $wx }
$srv = Get-ChildItem $wx -Recurse -Filter 'whisper-server.exe' | Select-Object -First 1
if (-not $srv) { throw 'whisper-server.exe not found in archive' }
Copy-Item (Join-Path $srv.DirectoryName '*.dll') $wbin -Force
Copy-Item $srv.FullName $wbin -Force
Remove-Item (Join-Path $wbin 'SDL2.dll') -ErrorAction SilentlyContinue  # only used by the live-mic demo tools

# --- llama.cpp (CPU, x64) ---
$lz = Join-Path $dl "llama-$LlamaTag.zip"
Get-File "https://github.com/ggml-org/llama.cpp/releases/download/$LlamaTag/llama-$LlamaTag-bin-win-cpu-x64.zip" $lz
$lx = Join-Path $dl "llama-$LlamaTag"
if (-not (Test-Path $lx)) { Expand-Archive $lz $lx }
$ls = Get-ChildItem $lx -Recurse -Filter 'llama-server.exe' | Select-Object -First 1
if (-not $ls) { throw 'llama-server.exe not found in archive' }
Copy-Item (Join-Path $ls.DirectoryName '*.dll') $lbin -Force
Copy-Item $ls.FullName $lbin -Force
# llama-server only needs its own impl; drop the other tools' DLLs to save space.
Get-ChildItem $lbin -Filter 'llama-*-impl.dll' | Where-Object Name -ne 'llama-server-impl.dll' | Remove-Item -Force

# --- models ---
$hf = 'https://huggingface.co'
Get-File "$hf/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en-q5_1.bin" (Join-Path $mdir 'ggml-tiny.en-q5_1.bin')
Get-File "$hf/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin" (Join-Path $mdir 'ggml-base.en-q5_1.bin')
Get-File "$hf/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf" (Join-Path $mdir 'qwen2.5-0.5b-instruct-q4_k_m.gguf')

Write-Host "`nDone. Resource sizes:"
Get-ChildItem $res -Recurse -File | Group-Object { $_.Directory.Name } | ForEach-Object {
    '{0,-10} {1,8:N1} MB' -f $_.Name, (($_.Group | Measure-Object Length -Sum).Sum / 1MB)
}
