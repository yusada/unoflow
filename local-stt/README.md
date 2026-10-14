# Local STT for UnoFlow

Run UnoFlow fully offline — no OpenAI key, no audio leaving your machine.

UnoFlow already speaks the OpenAI audio-transcriptions API, so pointing it at a
local server is a **settings change, not a code change**.

## Automatic start/stop

UnoFlow manages this server for you. If the configured **API base URL** points
at this machine (`127.0.0.1` / `localhost`), UnoFlow:

- **starts** `server.py` when the app launches (no console window), and
- **stops** it — process tree included — when the app exits.

So in local mode you just set the URL and forget about it; you don't need to
run anything by hand. UnoFlow finds the server in `local-stt/` next to the
executable, or in `%APPDATA%\UnoFlow\local-stt\`. If Python or the folder is
missing, it silently does nothing and expects a server to already be listening.

The manual instructions below still work if you'd rather run it yourself.

## Quick start (manual)

```powershell
cd local-stt
python -m pip install -r requirements.txt
python server.py
```

Then in UnoFlow: **Settings (⚙) → API base URL** = `http://127.0.0.1:8000/v1`,
leave the API key blank, and pick any model name (the local server ignores it
and uses its own).

## Which backend to use

There are two practical options. Pick based on whether you want a compiler.

| | `faster-whisper` (this folder) | `whisper.cpp` |
|---|---|---|
| Install | `pip install` (wheels) | needs CMake + a C++ toolchain |
| GPU | CUDA, needs cuDNN 9 DLLs | CUDA, one `.exe` |
| CPU | very good (CTranslate2) | good |
| Endpoint | `/v1/audio/transcriptions` (OpenAI-shaped) | `/inference` (different shape) |

**On this machine** (GTX 1060 3 GB, Ryzen 5 1600, 16 GB RAM, no CMake/MSVC/CUDA
toolkit): use **`faster-whisper` on CPU**. Building `whisper.cpp` would need
CMake + Visual Studio + the CUDA toolkit installed first, and the 3 GB card is
below the comfortable threshold for Whisper GPU inference.

## Model choice

Whisper needs roughly **~1 GB of VRAM for `small`, ~2 GB for `medium`** on GPU,
plus overhead. On a 3 GB card, `small` is the practical ceiling and `medium`
will usually fall back to CPU. Model sizes:

| Model | Disk | CPU speed | Notes |
|---|---|---|---|
| `tiny.en` | ~75 MB | fastest | lowest accuracy; fine for clear English |
| `base.en` | ~140 MB | fast | **server default** — good balance for dictation |
| `small.en` | ~460 MB | medium | noticeably better; use if CPU keeps up |
| `medium.en` | ~1.5 GB | slow on this CPU | GPU-recommended, won't fit well in 3 GB |
| `large-v3` | ~3 GB | too slow here | not realistic on this hardware |

The portable release bundles the multilingual `tiny` model (~75 MB), which is
UnoFlow's default — so it works offline immediately. The other sizes are
downloaded on first use.

English-only `.en` models are more accurate per size; drop the `.en` suffix for
multilingual.

Pick with an environment variable:

```powershell
$env:UNOFLOW_STT_MODEL = "small.en"
python server.py
```

The first run downloads the model (cached in `%USERPROFILE%\.cache\huggingface`).

### Standalone build (no Python needed)

The portable release ships `local-stt/unoflow-stt.exe` — a PyInstaller build of
this server with Python, `faster-whisper`, CTranslate2 and the VAD model all
bundled. **You do not need Python installed.** UnoFlow prefers this exe over
`server.py` when both are present, so the portable zip is zero-dependency.

Build it yourself (from this folder, with `pyinstaller` installed):

```powershell
py -3.10 -m pip install pyinstaller
py -3.10 -m PyInstaller --noconfirm --clean unoflow-stt.spec
```

Output: `dist/unoflow-stt/unoflow-stt.exe` (~320 MB with the model beside it —
PyInstaller one-folder, because a one-file build would unpack that on every
launch).

`server.py` remains the source of truth: the exe is just it, frozen. Run the
script directly when developing; ship the exe for users without Python.

### Bundled model (no download)

The **portable release zip** ships a pre-downloaded `tiny` model in
`local-stt/models/tiny/`. When that folder is present, the server loads it from
disk instead of downloading anything — so offline mode works on first run, with
no internet at all.

Resolution order in `server.py`:

1. `local-stt/models/<model>/model.bin` — the bundled copy, used if present.
2. Otherwise the model name is passed to faster-whisper, which downloads it from
   Hugging Face (cached as above).

To bundle a different model, copy its CTranslate2 files into
`local-stt/models/<name>/`:

```
local-stt/models/tiny/
├─ model.bin
├─ config.json
├─ tokenizer.json
└─ vocabulary.txt
```

Then set `UNOFLOW_STT_MODEL=tiny` (or pick it in UnoFlow's Settings). The
`models/` folder is gitignored — it is shipped as a release asset, not committed.


### Trying the GPU anyway

If you install the CUDA runtime + cuDNN 9 and set:

```powershell
$env:UNOFLOW_STT_DEVICE = "cuda"
$env:UNOFLOW_STT_COMPUTE = "float16"
python server.py
```

…`base`/`small` may fit in 3 GB. If you see out-of-memory errors, drop back to
CPU (`device=cpu`, `compute=int8`) — for short dictation clips the CPU path is
already fast enough.

## Multilingual

**Use the models without `.en`.** The `.en` variants are English-only; the plain
names are the multilingual ones (99 languages, auto-detected):

| Multilingual | English-only | Disk |
|---|---|---|
| `tiny` | `tiny.en` | ~75 MB |
| `base` | `base.en` | ~140 MB |
| `small` | `small.en` | ~460 MB |
| `medium` | `medium.en` | ~1.5 GB |

Leave **Language** blank to auto-detect, or set a code (`id`, `en`, `ja`, …) to
force one. Auto-detect works well: it identified Indonesian with 0.97–0.99
confidence in every test below.

### Measured accuracy — Indonesian, same clip

Real Indonesian audio (Edge TTS, `id-ID-ArdiNeural`), transcribed on CPU:

| Model | Detected | Result |
|---|---|---|
| `tiny` | `id` (0.97) | "Halo, namas saya budi. Saya sedemang uji aplikasi ini untuk menulis dengan suara dalam bahasa Indonesia." |
| `base` | `id` (0.99) | "Halo? Namah saya Budi. Saya sedang mengujah aplikasi ini untuk menolis dengan suara dalam bahasa Indonesia." |
| `small` | `id` (0.99) | "Halo, nama saya Budi. Saya sedang menguji aplikasi ini untuk menulis dengan suara dalam bahasa Indonesia." ✅ |

Reference text: *"Halo, nama saya Budi. Saya sedang menguji aplikasi ini untuk
menulis dengan suara dalam bahasa Indonesia."*

### Measured speed — same clip (~8 s of speech)

| Model | Time | vs real-time |
|---|---|---|
| `tiny` | 0.9–1.9 s | ~6× faster |
| `base` | 3.0 s | ~2.6× faster |
| `small` | 9.7 s | ~0.8× (slower than speech) |

**Pick based on what you care about:**

- **`tiny`** — fastest, good for quick notes. Expect small spelling slips.
- **`base`** — a reasonable middle. On this CPU it was not more accurate than
  `tiny` on the test clip, so try both.
- **`small`** — clearly the most accurate, but on a Ryzen 5 1600 it runs at
  roughly real-time, so a long dictation has a noticeable wait.

`medium` and `large-v3` are not realistic on this CPU (and `medium` won't fit
comfortably in a 3 GB GPU).

> Accuracy numbers above come from one synthetic TTS clip, which is cleaner than
> a real microphone and can be *harder* for Whisper than natural speech. Treat
> them as a rough ranking, not a benchmark.

## Tuning

| Env var | Default | Meaning |
|---|---|---|
| `UNOFLOW_STT_MODEL` | `base.en` | Whisper model name |
| `UNOFLOW_STT_DEVICE` | `cpu` | `cpu` or `cuda` |
| `UNOFLOW_STT_COMPUTE` | `int8` (cpu) / `float16` (cuda) | quantization |
| `UNOFLOW_STT_VAD` | `1` | voice-activity filter to skip silence |
| `UNOFLOW_STT_HOST` | `127.0.0.1` | bind address |
| `UNOFLOW_STT_PORT` | `8000` | bind port (UnoFlow sets this from your base URL) |

UnoFlow uses the `py` launcher and prefers the interpreter that already has
`faster_whisper` installed (here: `py -3.10`). If you install the dependency into
a different Python, update the probe order in `src-tauri/src/local_server.rs`.

## Measured latency

On this machine (Ryzen 5 1600, `base.en`, CPU int8), transcribing a **7.56 s**
clip:

| | |
|---|---|
| Cold (model loads on first request) | ~1.7 s |
| Warm | **~1.66 s** |

That's roughly **4.5× real-time**, i.e. faster than the speech itself — so a
dictation lands well before you'd notice. The model loads lazily on the first
request; UnoFlow waits for the server to accept connections before sending, so
you don't have to time your first dictation after launch.

## Test it without UnoFlow

```powershell
curl.exe -X POST http://127.0.0.1:8000/v1/audio/transcriptions `
  -F "file=@test.wav" -F "model=base.en"
```

Should return `{"text":"..."}`.

## Caveats

- **No streaming.** Like UnoFlow's cloud path, text arrives after you release
  the key. `faster-whisper` does have a streaming API if you want to wire it up.
- **The server is unauthenticated and binds to `127.0.0.1`.** It only accepts
  local connections; don't expose it.
- **`prompt` maps to `initial_prompt`**, so UnoFlow's context-awareness toggle
  works locally too — the focused control's text biases names and jargon.
