# Notes — what UnoFlow borrows from Wispr Flow

These are findings from static analysis of the installed Wispr Flow app
(`%LOCALAPPDATA%\WisprFlow\app-1.6.1147`) and its `Wispr Flow Helper.exe`.
They document *why* UnoFlow is shaped the way it is. All of this is
observation of shipped artifacts, not runtime verification.

## Proven from the app files

- **Stack:** Electron + webpack. Main bundle `.webpack/main/index.js`; 9 renderer
  entrypoints (contextMenu, overlay, hub, feature_tour, meeting_recorder,
  meeting_ax_inspector, scratchpad, status, accessibility_drop).
- **Native companion:** `resources\Release\Wispr Flow Helper.exe` (20 MB), a
  **.NET NativeAOT** binary (no CLI header — C# compiled to native, so managed
  decompilers report "not-managed").
- **Capture:** `getUserMedia` + `AudioWorklet` + bundled **Opus** encoders
  (`opusEncoder.worker.js`, `incrementalOpusEncoder.worker.js`). Encoder config
  seen in the WASM glue: `sampleRate 16000`, `channels 2`, `Application.AUDIO`,
  `bitrate 64000`. The recorder worklet buffers Float32 PCM (`chunkSize 640`).
- **Transcription:** gRPC `flow_api.v1.TranscriptionService` with `Transcribe`
  (unary) and `TranscribeStream` (bidirectional). Endpoint resolves to
  **Baseten** (`model-<id>.grpc.api.baseten.co`); an **OpenAI fallback** exists
  (`transcribeOpenAIAudioChunks` → `POST /llm/asr_audio_chunks`).
- **Context awareness:** the request carries `app`, `textbox`, `context`,
  `DynamicVocabulary`, `StyleConfig`, `EditingStrength` — so formatting and
  vocabulary correction happen **server-side**. The local DB stores `asrText`
  vs `formattedText` separately, confirming the split.
- **Insertion (the important part):** the helper imports `OpenClipboard`,
  `EmptyClipboard`, `SetClipboardData`, `GetClipboardData`,
  `GetClipboardSequenceNumber`, `RegisterClipboardFormat`, `SendInput`,
  `PostMessageW`, `GetForegroundWindow`, `SetForegroundWindow`, `EnumWindows`.
  Its method names include `PasteText`, `CancelActivePaste`,
  `FindDictatedTextInTextBox`, `FindDictatedTextInTextBoxWithRetry`,
  `GetSelectedTextViaCopy`, `GetTextBoxInfo`, `DisableClipboardRestore`,
  `ClipboardDelayedRendering`, `SimulateKeyPress`.
- **Accessibility:** `WisprFlowHelper_Uia_*` with `IUIAutomationElement`,
  `IUIAutomationTextPattern`, `IUIAutomationValuePattern`, plus
  `FocusChangeDetector` (text-changed / focus-changed handlers).
- **Audio session awareness:** `WisprFlowHelper_Wasapi_CoreAudioMicrophoneActivitySource_*`
  — WASAPI audio-session monitoring, which is how it knows which app holds the
  mic and detects meetings.
- **Persistence:** `node_sqlite3` → `%APPDATA%\Wispr Flow\flow.sqlite`.

## The insertion algorithm (reconstructed)

```
read focused control text (UIA)          # context + baseline
save current clipboard
write transcript to clipboard            # RegisterClipboardFormat for rich/HTML
simulate Ctrl+V (SendInput)
wait, then re-read the control
  if unchanged -> retry (FindDictatedTextInTextBoxWithRetry) / Shift+Insert / CancelActivePaste
restore the saved clipboard              # unless DisableClipboardRestore
record PasteOutcome (verified | unverified | blocked)
```

### Confirmed by Ghidra decompilation (12.1.4, 57,023 procedures)

Method names recovered from the NativeAOT binary's metadata blobs:

- `PasteText`, `PasteTextHelper`, `PasteTextInternal`
- `CopyWithFormatting`, `ConvertToWindowsClipboardHtmlFormat`
- `TrySetClipboardWithVerification`
- `FindDictatedTextInTextBox`, `FindDictatedTextInTextBoxWithRetry`
- `GetSelectedTextViaCopy`
- `SendPasteOutcomeRequest`
- `SimulateKeyPress` / `Exception in SimulateKeyPressHandler`
- `DisableClipboardRestore`

Log strings recovered (these are the smoking gun):

- `"Beginning pasteText with delimiters."`
- `"Beginning regular pasteText."`
- `"DisableClipboardRestore is enabled, not restoring clipboard contents"`
- `"CouldNotGetTextBoxInfo"`

This confirms the reconstruction: the helper writes the transcript to the
clipboard (in a plain *and* an HTML/rich format — "with delimiters" vs
"regular"), simulates the paste, verifies against the text box, retries via
`FindDictatedTextInTextBoxWithRetry`, and restores the clipboard unless
`DisableClipboardRestore` is set. UnoFlow implements the plain-text branch of
the same design.

UnoFlow implements the same skeleton: plain-text clipboard write, `Ctrl+V`, a
retry ladder (`Ctrl+V` retries then `Shift+Insert`), UIA verification, and
unconditional clipboard restore. It does not implement the rich/HTML clipboard
formats.

## Where REA got stuck (honest limits)

- `analyze-javascript-application` on the 130 MB `app.asar` returned a result
  with `coverage.truncated: true` (budget-limited). It found the structure —
  207 IPC operations, 36 contextBridge members, 9 renderers, 5 native addons —
  but **could not pair renderer IPC calls to their `ipcMain` handlers**
  (`paired_renderer_transmissions: 0`, `108 unpaired`).
- `trace-application-feature` on the `simulateKeyPress` channel returned
  `no-match` because the channel literals were not retained in the truncated
  graph. REA correctly reported this as "not evidence of absence."
- The helper is NativeAOT, so REA's managed provider said `not-managed`. Native
  analysis needs Ghidra/IDA. (Ghidra 12.1.4 + Temurin JDK 21 were installed to
  enable this; `rea doctor` reports the Ghidra provider healthy.)

## Deliberately not copied

- Server-side formatting/LLM cleanup and vocabulary learning.
- Notetaker / meeting capture and WASAPI session monitoring.
- Jabra headset SDK, BLE ring, PostHog/Segment/Sentry telemetry, Supabase auth.
