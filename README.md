# Utter

Free, open-source, **100% offline** voice dictation for your desktop. Hold a key, speak, let go: clean text appears wherever your cursor is.

No account, no API keys, no internet. Your voice never leaves your computer.

## How to use

| Action | Keys |
| --- | --- |
| Talk | Hold **Ctrl + Win**, speak, release |
| Hands-free | Hold **Ctrl + Win**, press **Space**, then let go. Press **Ctrl + Win** again to finish |
| Cancel | **Esc** while listening |

The text is typed at your cursor **and** stays on your clipboard, so you can paste it again.

A small indicator at the bottom of the screen shows when Utter is listening or working. Everything else lives in the tray icon → **Settings**:

- **Push-to-talk keys**: press any combo to record it, including modifier-only combos like Ctrl+Alt.
- **Dictionary**: names and jargon Utter should listen for and always spell your way, plus replacements ("jira" → "Jira").
- **Snippets**: say a trigger on its own ("my email") and Utter types the full text.
- **App tones**: set any app (or a word in a window title, like "gmail") to Casual, Formal, Neutral or Code.
- **History**: search past dictations, then copy, re-paste or delete them. They're stored only on your PC and deleted after 30 days.
- Fast Mode, AI cleanup, idle indicator, and Start with Windows (on by default).

## What it does to your words

- Removes fillers: "um", "uh", "hmm", stutters ("I I think").
- Understands corrections: "let's meet at 2… actually 3" → "Let's meet at 3."
- "Scratch that" deletes the previous sentence.
- Spoken lists become numbered lists: "one, eggs, two, milk, three, bread".
- "New line" / "new paragraph".
- Adapts to the app: casual in Slack/Discord/WhatsApp, polished in Outlook/Gmail, untouched in code editors and terminals.
- Optional **AI cleanup** with a small local model (Qwen2.5 0.5B) fixes grammar and punctuation. Its output is checked against what you said and discarded if it changed your meaning.

## Runs on modest hardware

On first launch Utter checks your RAM and CPU:

| Your PC | Speech model | AI cleanup |
| --- | --- | --- |
| Under 6 GB RAM or 2 cores | Whisper tiny (Fast Mode) | off |
| 6–8 GB RAM | Whisper base | off |
| 8 GB+ RAM, 4+ cores | Whisper base | on |

You can switch at any time from the tray menu. **Fast Mode** uses the tiny model and rules-only cleanup, for near-instant results on old laptops.

Memory use while idle is about 400 MB in total: the app, the Whisper speech engine (~120 MB), and the indicator window. The AI cleanup model (~500 MB) loads only while you dictate. It starts loading the moment you press the hotkey, so it's ready when you stop talking, and it's unloaded after 5 minutes without use. Idle CPU is near zero.

## Install

Download the installer from Releases and run it. No admin rights are needed.

> The installer is not code-signed yet, so Windows SmartScreen may say "Windows protected your PC". Click **More info → Run anyway**.

### Known limits

- Windows only for now. macOS and Linux are planned.
- Windows blocks normal apps from typing into apps running **as administrator**. Run Utter as admin too if you need that.
- English only in this version.

## Build from source

Requirements: [Rust](https://rustup.rs), Node.js 18+, and Visual Studio C++ Build Tools.

```powershell
npm install
npm run fetch-deps   # downloads whisper.cpp, llama.cpp and the models (~620 MB)
npm run dev          # run in development
npm run build        # make the installer in src-tauri/target/release/bundle/nsis
```

Test the speech pipeline on a WAV file without the hotkey:

```powershell
src-tauri\target\debug\utter.exe --transcribe sample.wav [slack.exe]
```

Logs: `%LOCALAPPDATA%\app.utter.dictation\logs\utter.log`

## How it works

```
Ctrl+Win (keyboard hook) → mic (cpal, 16 kHz) → whisper-server (local)
  → rules cleanup → optional llama-server polish (local) → clipboard + Ctrl+V
```

- **Tauri 2 (Rust)**: small native shell, tray, and the indicator window.
- **whisper.cpp** and **llama.cpp** run as hidden local helper processes bound to `127.0.0.1`. Models stay loaded, so each dictation only pays for inference. They are closed automatically when Utter exits.

## Credits

[whisper.cpp](https://github.com/ggml-org/whisper.cpp) and [llama.cpp](https://github.com/ggml-org/llama.cpp) (MIT), OpenAI Whisper models (MIT), Qwen2.5 (Apache 2.0), Tauri (MIT/Apache 2.0).

## License

MIT
