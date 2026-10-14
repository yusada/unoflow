# UnoFlow

**Minimal voice-to-text dictation for Windows.** Hold a hotkey, speak, release —
the transcript is pasted into whatever app has focus.

UnoFlow is a small, self-contained desktop app built with **Tauri v2** (a Rust
core + a WebView2 frontend). It runs against OpenAI's transcription API by
default, or **fully offline** against a bundled local Whisper server — your
choice, one setting apart. No account, no telemetry; the only network call is
the transcription request itself.

```
hold Ctrl+Shift+Space ──► speak ──► release ──► text appears in the focused app
```

---

## Highlights

- **Push-to-talk dictation** with a configurable global hotkey.
- **Pastes into any app** — uses the clipboard + a simulated paste, not
  character-by-character typing, so it works in editors, browsers, terminals,
  and IMEs alike.
- **Verified insertion** — after pasting, UnoFlow re-reads the focused control
  (UI Automation) and reports whether the text actually landed.
- **Cloud or local** — point it at OpenAI, or at the bundled `faster-whisper`
  server and keep every byte of audio on your machine.
- **Multilingual** — 99 languages with Whisper's multilingual models, with
  optional language forcing and auto-detection.
- **Transcript cleanup** — optionally strips filler words ("um", "uh") and
  collapses repeated words.
- **Dictionary** — a list of names/jargon sent as a prompt so the model spells
  them right.
- **Snippets** — trigger → expansion pairs, applied to the transcript.
- **Translation** — dictate in one language, paste in another (needs a chat
  endpoint).
- **History & Insights** — searchable transcript history plus local stats:
  words-per-minute, day streak, a 14-day activity chart, the apps you dictate
  into, and your most-used words.
- **Floating status pill** — a small on-screen capsule while dictating, so you
  can see "Listening… / Transcribing…" without leaving your app.

---

## How it works

```
global hotkey (Ctrl+Shift+Space)
        │
        ▼
WebView: getUserMedia → AudioWorklet → Float32 PCM
        │  (downsample 48k → 16k, encode PCM16, send to Rust)
        ▼
Rust: POST /v1/audio/transcriptions  (OpenAI-compatible, multipart WAV)
        │
        ▼
Rust: cleanup → snippet expansion → optional translation
        │
        ▼
Rust: save clipboard → set clipboard → SendInput Ctrl+V → restore clipboard
        │  UIA read of the focused control verifies the paste landed
        ▼
text appears in the focused app
```

Three parts carry the weight, and each is done deliberately:

1. **Capture lives in the WebView.** `getUserMedia` + an `AudioWorklet` run at
   the device rate (typically 48 kHz); Rust receives 16 kHz PCM16. Resampling in
   JS is fine — it isn't real-time critical.
2. **Insertion is clipboard + paste, not typing.** Typing char-by-char is slow
   and breaks in IMEs and terminals. UnoFlow sets the clipboard, sends `Ctrl+V`
   (falling back to `Shift+Insert` where needed), then puts your clipboard back.
3. **Verification via UI Automation.** After pasting, UnoFlow re-reads the
   focused control and checks the text is there, so a failed paste is reported
   instead of silently pretending to work.

> **Design note.** UnoFlow is a clean-room implementation of the same
> architecture the commercial tool *Wispr Flow* uses — capture in the WebView,
> a native helper for insertion, UIA for verification. [`NOTES.md`](NOTES.md)
> documents the static analysis behind that design and, just as importantly,
> what UnoFlow deliberately leaves out. It is not affiliated with Wispr Flow.

---

## Requirements

- **Windows 10/11** — text insertion is Windows-only (`paste.rs` is `cfg(windows)`).
- **Rust** (stable) with the `x86_64-pc-windows-msvc` (or `-gnu`) toolchain.
- **WebView2 runtime** — preinstalled on Windows 11 and current Windows 10.
- **Tauri CLI v2** — installed via `npm install` (pinned in `package.json`).
- **Node.js** — only to run the Tauri CLI.
- **Optional, for offline mode:** Python 3.10+ with `faster-whisper`.

---

## Getting started

```powershell
git clone https://github.com/yusada/unoflow.git
cd unoflow
npm install

npm run dev      # development, hot reload
npm run build    # NSIS installer under src-tauri/target/release/bundle
```

The plain binary lands at `src-tauri/target/release/unoflow.exe`.

> **Building with `cargo build` instead of `cargo tauri build`?** You must also
> copy `src-tauri/target/release/WebView2Loader.dll` next to `unoflow.exe`.
> Without it the app dies at startup with *"WebView2Loader.dll was not found"*.
> The Tauri CLI does this for you; a raw `cargo build` does not.

---

## Configuration

Open **Settings (⚙)** in the app. Values persist to
`%APPDATA%\UnoFlow\config.json`.

| Setting | What it does |
|---|---|
| **Shortcut** | The global push-to-talk hotkey (default `Ctrl+Shift+Space`). Click **Record**, then press your keys. |
| **API base URL** | Where transcription requests go. OpenAI by default, or a local server. |
| **API key** | Your OpenAI key. Leave blank for a local server. |
| **Model** | `gpt-4o-mini-transcribe` / `whisper-1` (cloud) or a Whisper size (local). |
| **Language** | Blank = auto-detect (multilingual models only), or a code like `en`, `id`. |
| **Transcript cleanup** | Off / Light (drop fillers, tidy punctuation) / Strong (also collapse repeats). |
| **Translate dictation** | Speak in one language, paste in another. |
| **Show floating pill** | The on-screen status capsule while dictating. |
| **Context awareness** | Send the focused app's text to the model to improve names/jargon. |

The API key can also come from an environment variable:

```powershell
$env:UNOFLOW_OPENAI_KEY = "sk-..."
```

> **Renamed from `FLOWLITE_*`.** Earlier builds used `FLOWLITE_OPENAI_KEY` and
> `FLOWLITE_STT_*`. If you set those, rename them to `UNOFLOW_*` — the old names
> are no longer read.

---

## Cloud mode (OpenAI)

Out of the box, set the **API base URL** to `https://api.openai.com/v1`, paste
your key, and pick a model. Pricing is per minute of audio (see OpenAI's page);
`whisper-1` is about $0.006/min.

## Local / offline mode

Run UnoFlow with **no API key and no audio leaving your machine**. Point
**Settings → API base URL** at a local server:

```
http://127.0.0.1:8000/v1
```

Leave the API key blank (an empty key is only rejected for `api.openai.com`).
UnoFlow then **starts the bundled server on launch and stops it on exit**, and
waits for it to accept connections before sending the first request — so a
dictation right after launch still works. You don't manage anything by hand.

The server lives in [`local-stt/`](local-stt/README.md), which covers setup,
model choice, measured accuracy/latency, and the GPU/CPU trade-off on modest
hardware. Quick version:

```powershell
cd local-stt
python -m pip install -r requirements.txt
python server.py
```

Two backends are practical — `faster-whisper` (this repo, pip-installable,
no compiler) or `whisper.cpp` (needs CMake + a C++ toolchain). See
[`local-stt/README.md`](local-stt/README.md) for the comparison.

> **No first-run download.** The portable release zip ships the `tiny` model
> (~75 MB) in `local-stt/models/`, so offline mode works immediately with no
> internet. A source checkout downloads the model on first use instead (cached
> in `%USERPROFILE%\.cache\huggingface`).

> The local server is unauthenticated and binds to `127.0.0.1`. It only accepts
> local connections — don't expose it.

---

## Usage

UnoFlow has four pages in the left sidebar:

- **Dictation** — the hero card, your transcript history (search, copy, delete),
  and live stats. Dictate with the hotkey; results land in the focused app and
  appear in the list.
- **Insights** — totals, words-per-minute, day streak, a 14-day activity chart,
  the apps you dictate into, and your most-used words. All computed locally.
- **Dictionary** — words and names the model should get right. Stored locally and
  sent as a prompt (`Vocabulary: …`) with each transcription.
- **Snippets** — trigger → expansion pairs. Stored locally and applied to the
  transcript after transcription.

Everything is stored in `%APPDATA%\UnoFlow\store.json`; nothing leaves the
machine except the transcription request itself.

---

## Project layout

```
unoflow/
├─ package.json                 # only the Tauri CLI
├─ src/                         # frontend (no bundler; plain ES modules)
│  ├─ index.html                # main window: sidebar, pages, settings modal
│  ├─ styles.css
│  ├─ app.js                    # capture + orchestration + UI
│  ├─ recorder-worklet.js       # AudioWorkletProcessor (audio thread)
│  └─ pill.html / pill.js       # floating status capsule
├─ src-tauri/
│  ├─ Cargo.toml
│  ├─ tauri.conf.json           # windows, tray icon, bundle config
│  ├─ capabilities/default.json
│  ├─ icons/
│  └─ src/
│     ├─ main.rs                # hotkey, tray, config, commands, orchestration
│     ├─ transcribe.rs          # WAV wrapping + OpenAI-compatible request
│     ├─ paste.rs               # clipboard + SendInput + UIA (Windows)
│     ├─ store.rs               # history, snippets, dictionary, insights
│     ├─ cleanup.rs             # filler removal, tidy, snippet expansion
│     ├─ translate.rs           # chat-completions translation
│     ├─ local_server.rs        # start/stop the bundled Whisper server
│     └─ appinfo.rs             # which app has focus
└─ local-stt/                   # offline Whisper server (Python)
   ├─ server.py
   ├─ requirements.txt
   ├─ README.md
   └─ models/                  # bundled Whisper weights (release zip only)
```

---

## Swapping the transcription service

`transcribe_pcm` in `src-tauri/src/transcribe.rs` is the single place the audio
leaves the app. To use something else:

- **Local/private:** run `whisper.cpp`'s `whisper-server` and point the request
  at `http://127.0.0.1:8080/inference` with the same WAV body.
- **Streaming (lower latency):** Deepgram/AssemblyAI/Google all have WebSocket
  APIs; feed them the PCM chunks from the worklet directly instead of waiting
  for release. The current design is batch-on-release, which is simpler and
  plenty for short dictation.

---

## Known limitations

- **Windows only.** macOS/Linux would need `AXUIElement`/`CGEventPost` and
  `xdotool`/`uinput` equivalents for insertion.
- **Elevated targets.** A non-elevated UnoFlow cannot paste into an app running
  as Administrator (UIPI blocks `SendInput`). Run UnoFlow elevated if you need
  that, or accept the failed-paste notification.
- **Batch, not streaming.** Text appears after you release the key, not live.
- **No server-side context awareness.** The focused control's text is read
  locally and passed as a prompt, but there is no server-side formatting/LLM
  cleanup or vocabulary learning (that's a deliberate omission — see `NOTES.md`).
- **Clipboard timing.** Some slow apps need a longer delay than the 140 ms used
  before verification; it's a single constant (`INITIAL_SETTLE_MS`) in `paste.rs`.

---

## Services it needs

| Service | Required? | Notes |
|---|---|---|
| OpenAI audio-transcriptions API | yes, unless local | ~$0.006/min for `whisper-1` |
| Local `whisper.cpp` / `faster-whisper` server | optional | replaces OpenAI, keeps audio on-device |
| Chat endpoint (`/chat/completions`) | only for translation | OpenAI-compatible |
| Account/sync backend | no | add later if you want cross-device history |

No telemetry, no analytics, no account.

---

## License

[MIT](LICENSE)

## Acknowledgements

- [Tauri](https://tauri.app/) — the desktop shell.
- [faster-whisper](https://github.com/SYSTRAN/faster-whisper) — offline STT.
- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) — alternative local STT.
- The insertion design is inspired by analysis of *Wispr Flow*; see
  [`NOTES.md`](NOTES.md). UnoFlow is unaffiliated with it.
