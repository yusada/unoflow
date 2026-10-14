// UnoFlow frontend: capture mic audio, hand PCM to Rust for transcription,
// then ask Rust to paste it into the focused app. Also renders history,
// dictionary, snippets, and settings.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const TARGET_RATE = 16000;
const DEFAULT_BASE_URL = "https://api.openai.com/v1";
const MAX_CONTEXT_CHARS = 2000;

// ---------- elements ----------
const $ = (id) => document.getElementById(id);
const dot = $("dot");
const statusEl = $("status");
const toastEl = $("toast");

// ---------- state ----------
let audioCtx = null;
let micStream = null;
let workletNode = null;
let chunks = [];
let recording = false;
let recordingStartedAt = 0;
let lastText = "";
let dictionary = [];
let snippets = [];

// ---------- helpers ----------
function toast(msg, ms = 1800) {
  toastEl.textContent = msg;
  toastEl.classList.remove("hidden");
  clearTimeout(toast._t);
  toast._t = setTimeout(() => toastEl.classList.add("hidden"), ms);
}

function setState(state, message) {
  dot.dataset.state = state;
  if (message) statusEl.textContent = message;
  invoke("set_status", { status: state }).catch(() => {});
}

function fmtTime(ts) {
  const d = new Date(ts);
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function fmtDay(ts) {
  const d = new Date(ts);
  const today = new Date();
  const same = d.toDateString() === today.toDateString();
  return same ? "Today" : d.toLocaleDateString([], { month: "short", day: "numeric" });
}

/** Average-downsample Float32 mono from `inRate` to `outRate`. */
function downsample(input, inRate, outRate) {
  if (inRate === outRate) return input;
  const ratio = inRate / outRate;
  const outLen = Math.round(input.length / ratio);
  const out = new Float32Array(outLen);
  let pos = 0;
  for (let i = 0; i < outLen; i++) {
    const next = Math.round((i + 1) * ratio);
    let sum = 0;
    let n = 0;
    for (; pos < next && pos < input.length; pos++) {
      sum += input[pos];
      n++;
    }
    out[i] = n ? sum / n : 0;
  }
  return out;
}

/** Float32 [-1,1] -> little-endian PCM16 bytes. */
function toPcm16(float32) {
  const buf = new ArrayBuffer(float32.length * 2);
  const view = new DataView(buf);
  for (let i = 0; i < float32.length; i++) {
    const s = Math.max(-1, Math.min(1, float32[i]));
    view.setInt16(i * 2, s < 0 ? s * 0x8000 : s * 0x7fff, true);
  }
  return new Uint8Array(buf);
}

/** Focused-app text, capped, only when context awareness is on. */
async function focusedContext() {
  try {
    const t = await invoke("read_focused_text");
    return (t || "").trim().slice(0, MAX_CONTEXT_CHARS);
  } catch {
    return "";
  }
}

// ---------- recording ----------
// Guards against a second start firing while the first is still awaiting the
// mic and worklet setup. `recording` is only set at the end of startRecording,
// so without this a double event would open two capture graphs.
let starting = false;

async function startRecording() {
  if (recording || starting) return;
  starting = true;
  try {
    micStream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true },
    });
  } catch (e) {
    starting = false;
    setState("idle", "Microphone unavailable");
    toast("Could not open the microphone: " + e.message);
    return;
  }

  audioCtx = new AudioContext();
  await audioCtx.audioWorklet.addModule("recorder-worklet.js");
  const source = audioCtx.createMediaStreamSource(micStream);
  workletNode = new AudioWorkletNode(audioCtx, "recorder-processor");

  chunks = [];
  workletNode.port.onmessage = (e) => {
    if (e.data && e.data.samples) chunks.push(e.data.samples);
  };
  source.connect(workletNode);
  workletNode.connect(audioCtx.destination); // keep the graph alive

  workletNode.port.postMessage("start");
  recording = true;
  starting = false;
  recordingStartedAt = Date.now();
  setState("recording", "Listening… release to transcribe");
}

function stopGraph() {
  try {
    workletNode && workletNode.port.postMessage("stop");
    workletNode && workletNode.disconnect();
  } catch {}
  try {
    micStream && micStream.getTracks().forEach((t) => t.stop());
  } catch {}
  const ctx = audioCtx;
  audioCtx = null;
  workletNode = null;
  micStream = null;
  if (ctx) ctx.close().catch(() => {});
}

async function stopAndTranscribe() {
  if (!recording) return;
  recording = false;
  const durationMs = Date.now() - recordingStartedAt;
  const inRate = audioCtx ? audioCtx.sampleRate : TARGET_RATE;
  stopGraph();

  if (chunks.length === 0) {
    setState("idle", "No audio captured");
    return;
  }
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const merged = new Float32Array(total);
  let off = 0;
  for (const c of chunks) {
    merged.set(c, off);
    off += c.length;
  }
  chunks = [];

  const down = downsample(merged, inRate, TARGET_RATE);
  const pcm = Array.from(toPcm16(down));

  setState("processing", "Transcribing…");
  try {
    const context = await focusedContext();
    const text = await invoke("transcribe", { pcm, sampleRate: TARGET_RATE, context, durationMs: durationMs });
    if (!text) {
      setState("idle", "Nothing recognised");
      return;
    }
    lastText = text;
    const ok = await invoke("insert_text", { text });
    setState("done", ok ? "Inserted" : "Pasted (unverified)");
    toast(ok ? "Inserted" : "Pasted — could not confirm it landed");
    // The Rust side records history; refresh the list.
    await loadHistory();
    await loadStats();
    if (!$("view-insights").classList.contains("hidden")) await loadInsights();
    setTimeout(() => setState("idle", "Idle"), 1500);
  } catch (e) {
    setState("idle", "Transcription failed");
    toast(String(e), 4000);
  }
}

// ---------- history ----------
async function loadHistory() {
  const list = $("historyList");
  const entries = await invoke("list_history");
  const q = ($("historySearch").value || "").toLowerCase();
  const shown = q ? entries.filter((e) => e.text.toLowerCase().includes(q)) : entries;

  list.innerHTML = "";
  $("historyEmpty").classList.toggle("hidden", shown.length > 0);
  $("historyEmpty").textContent = q ? "No matches." : "No dictations yet.";

  for (const e of shown) {
    const li = document.createElement("li");
    const words = e.text.trim().split(/\s+/).length;

    const time = document.createElement("span");
    time.className = "time";
    time.textContent = fmtTime(e.ts);
    time.title = `${fmtDay(e.ts)} · ${words} words`;

    const body = document.createElement("div");
    body.className = "body";
    const text = document.createElement("div");
    text.className = "text";
    text.textContent = e.text;
    body.appendChild(text);

    const acts = document.createElement("div");
    acts.className = "acts";
    const copy = document.createElement("button");
    copy.className = "icon-btn";
    copy.title = "Copy";
    copy.textContent = "⧉";
    copy.onclick = () => {
      navigator.clipboard.writeText(e.text);
      toast("Copied");
    };
    const del = document.createElement("button");
    del.className = "icon-btn";
    del.title = "Delete";
    del.textContent = "🗑";
    del.onclick = async () => {
      await invoke("delete_history", { id: e.id });
      await loadHistory();
      await loadStats();
    };
    acts.append(copy, del);

    li.append(time, body, acts);
    list.appendChild(li);
  }
}

async function loadStats() {
  const s = await invoke("get_stats");
  $("statWords").textContent = s.total_words;
  $("statWpm").textContent = s.wpm;
  $("statStreak").textContent = s.day_streak;
}

// ---------- dictionary ----------
async function loadDictionary() {
  dictionary = await invoke("list_dictionary");
  const list = $("dictList");
  list.innerHTML = "";
  $("dictEmpty").classList.toggle("hidden", dictionary.length > 0);
  dictionary.forEach((word, i) => {
    const li = document.createElement("li");
    const span = document.createElement("span");
    span.textContent = word;
    const rm = document.createElement("button");
    rm.textContent = "✕";
    rm.title = "Remove";
    rm.onclick = async () => {
      dictionary.splice(i, 1);
      await invoke("set_dictionary", { words: dictionary });
      await loadDictionary();
    };
    li.append(span, rm);
    list.appendChild(li);
  });
}

// ---------- snippets ----------
async function loadSnippets() {
  snippets = await invoke("list_snippets");
  const list = $("snipList");
  list.innerHTML = "";
  $("snipEmpty").classList.toggle("hidden", snippets.length > 0);

  snippets.forEach((s, i) => {
    const tr = document.createElement("tr");
    const c1 = document.createElement("td");
    c1.className = "trig";
    c1.textContent = s.trigger;
    const c2 = document.createElement("td");
    const arrow = document.createElement("span");
    arrow.className = "arrow";
    arrow.textContent = "→";
    const exp = document.createElement("span");
    exp.textContent = s.expansion;
    c2.append(arrow, exp);
    const c3 = document.createElement("td");
    c3.className = "acts";
    const del = document.createElement("button");
    del.className = "icon-btn";
    del.textContent = "🗑";
    del.onclick = async () => {
      snippets.splice(i, 1);
      await invoke("set_snippets", { snippets });
      await loadSnippets();
    };
    c3.appendChild(del);
    tr.append(c1, c2, c3);
    list.appendChild(tr);
  });
}

// ---------- insights ----------
async function loadInsights() {
  const d = await invoke("get_insights");
  $("iWords").textContent = d.total_words;
  $("iEntries").textContent = d.total_entries;
  $("iWpm").textContent = d.wpm;
  $("iStreak").textContent = d.day_streak;
  $("iAvg").textContent = d.avg_words;
  $("iMinutes").textContent = d.total_minutes;

  const max = Math.max(1, ...d.daily.map((x) => x.words));
  const bars = $("iBars");
  bars.innerHTML = "";
  for (const day of d.daily) {
    const el = document.createElement("div");
    el.className = "bar" + (day.words > 0 ? " has" : "");
    el.style.height = Math.max(3, (day.words / max) * 118) + "px";
    const label = new Date(day.day * 86400000).toLocaleDateString([], { month: "short", day: "numeric" });
    el.dataset.tip = `${label}: ${day.words} words`;
    el.title = el.dataset.tip;
    bars.appendChild(el);
  }

  const apps = $("iApps");
  apps.innerHTML = "";
  $("iAppsEmpty").classList.toggle("hidden", d.apps.length > 0);
  for (const a of d.apps) {
    const li = document.createElement("li");
    const n = document.createElement("span");
    n.className = "name";
    n.textContent = a.app;
    const v = document.createElement("span");
    v.className = "n";
    v.textContent = `${a.words} w`;
    li.append(n, v);
    apps.appendChild(li);
  }

  const words = $("iWords2");
  words.innerHTML = "";
  if (d.top_words.length === 0) {
    const li = document.createElement("li");
    li.textContent = "No data yet.";
    li.style.background = "transparent";
    li.style.border = "0";
    words.appendChild(li);
  }
  for (const [w, n] of d.top_words) {
    const li = document.createElement("li");
    const span = document.createElement("span");
    span.textContent = w;
    const badge = document.createElement("span");
    badge.className = "n";
    badge.textContent = n;
    badge.style.color = "var(--muted)";
    li.append(span, badge);
    words.appendChild(li);
  }
}

// ---------- views ----------
function showView(name) {
  for (const v of document.querySelectorAll(".view")) v.classList.add("hidden");
  $("view-" + name).classList.remove("hidden");
  for (const b of document.querySelectorAll(".nav-item[data-view]")) {
    b.classList.toggle("active", b.dataset.view === name);
  }
  if (name === "insights") loadInsights();
}

// ---------- shortcut recording ----------
// Turns a keydown event into the "Ctrl+Shift+Space" form the Rust side parses.
const MOD_ORDER = ["Ctrl", "Shift", "Alt", "Super"];

function eventToShortcut(e) {
  const mods = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.shiftKey) mods.push("Shift");
  if (e.altKey) mods.push("Alt");
  if (e.metaKey) mods.push("Super");

  const k = e.key;
  // Modifier-only presses aren't a shortcut yet.
  if (["Control", "Shift", "Alt", "Meta"].includes(k)) return null;

  let key;
  if (k === " ") key = "Space";
  else if (k.length === 1) key = k.toUpperCase();
  else if (/^F\d{1,2}$/.test(k)) key = k; // F1..F12
  else if (k === "Escape") key = "Escape";
  else if (k === "Enter") key = "Enter";
  else if (k === "Tab") key = "Tab";
  else if (k === "Backspace") key = "Backspace";
  else if (k.startsWith("Arrow")) key = k.slice(5); // Up/Down/Left/Right
  else key = k;

  // Require at least one modifier unless it's a function key.
  if (mods.length === 0 && !/^F\d{1,2}$/.test(key)) return null;

  return [...MOD_ORDER.filter((m) => mods.includes(m)), key].join("+");
}

let recordingShortcut = false;

function beginShortcutRecording() {
  recordingShortcut = true;
  $("shortcutInput").value = "Press keys…";
  $("shortcutInput").closest(".shortcut-edit").classList.add("recording");
}

function endShortcutRecording() {
  recordingShortcut = false;
  $("shortcutInput").closest(".shortcut-edit").classList.remove("recording");
}

$("recordShortcut").onclick = () => {
  if (recordingShortcut) return endShortcutRecording();
  beginShortcutRecording();
};

// Capture keys while recording. Registered on the document in the capture
// phase so it wins over anything else in the dialog.
document.addEventListener(
  "keydown",
  (e) => {
    if (!recordingShortcut) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      endShortcutRecording();
      $("shortcutInput").value = $("shortcutInput").dataset.current || "";
      return;
    }
    const combo = eventToShortcut(e);
    if (combo) {
      $("shortcutInput").value = combo;
      endShortcutRecording();
    }
  },
  true
);

// ---------- settings ----------
async function openSettings() {
  const cfg = await invoke("get_config");
  const sc = $("shortcutInput");
  sc.value = cfg.shortcut || "Ctrl+Shift+Space";
  sc.dataset.current = sc.value;
  endShortcutRecording();
  $("baseUrl").value = cfg.base_url || DEFAULT_BASE_URL;
  $("apiKey").value = "";
  $("apiKey").placeholder = cfg.api_key_set ? "•••• (saved)" : "sk-…";
  $("model").value = cfg.model;
  if (!$("model").value) $("model").value = cfg.model; // keep custom value visible
  $("language").value = cfg.language || "";
  $("contextAware").checked = !!cfg.context_aware;
  $("showPill").checked = cfg.show_pill !== false;
  $("cleanupLevel").value = String(cfg.cleanup_level ?? 1);
  $("translateEnabled").checked = !!cfg.translate_enabled;
  $("translateTo").value = cfg.translate_to || "English";
  $("translateModel").value = cfg.translate_model || "gpt-4o-mini";
  $("settingsOverlay").classList.remove("hidden");
}

async function saveSettings() {
  const key = $("apiKey").value.trim();
  await invoke("set_config", {
    apiKey: key || null,
    baseUrl: $("baseUrl").value.trim() || DEFAULT_BASE_URL,
    model: $("model").value,
    language: $("language").value.trim(),
    contextAware: $("contextAware").checked,
    showPill: $("showPill").checked,
    cleanupLevel: Number($("cleanupLevel").value),
    translateEnabled: $("translateEnabled").checked,
    translateTo: $("translateTo").value.trim() || "English",
    translateModel: $("translateModel").value.trim() || "gpt-4o-mini",
  });

  // The shortcut is applied separately so we can report a registration failure.
  const sc = $("shortcutInput").value.trim();
  if (sc && sc !== $("shortcutInput").dataset.current) {
    try {
      await invoke("set_shortcut", { shortcut: sc });
      $("shortcutInput").dataset.current = sc;
      toast("Shortcut updated");
    } catch (e) {
      toast(String(e), 4000);
      $("shortcutInput").value = $("shortcutInput").dataset.current || "";
    }
  }

  $("settingsOverlay").classList.add("hidden");
  toast("Settings saved");
}

// ---------- wiring ----------
$("openSettings").onclick = openSettings;
$("settingsCancel").onclick = () => $("settingsOverlay").classList.add("hidden");
$("settingsSave").onclick = saveSettings;
$("settingsOverlay").onclick = (e) => {
  if (e.target === $("settingsOverlay")) $("settingsOverlay").classList.add("hidden");
};

for (const b of document.querySelectorAll(".nav-item[data-view]")) {
  b.onclick = () => showView(b.dataset.view);
}

$("historySearch").oninput = loadHistory;
$("clearHistory").onclick = async () => {
  await invoke("clear_history");
  await loadHistory();
  await loadStats();
  toast("History cleared");
};

$("heroCopy").onclick = () => {
  if (!lastText) return;
  navigator.clipboard.writeText(lastText);
  toast("Copied");
};
$("heroInsert").onclick = async () => {
  if (!lastText) return;
  const ok = await invoke("insert_text", { text: lastText });
  toast(ok ? "Inserted" : "Pasted — could not confirm");
};

$("dictAdd").onclick = async () => {
  const v = $("dictInput").value.trim();
  if (!v || dictionary.includes(v)) return;
  dictionary.push(v);
  await invoke("set_dictionary", { words: dictionary });
  $("dictInput").value = "";
  await loadDictionary();
};
$("dictInput").addEventListener("keydown", (e) => {
  if (e.key === "Enter") $("dictAdd").click();
});

$("snipAdd").onclick = async () => {
  const trigger = prompt("Trigger (what you say):");
  if (!trigger) return;
  const expansion = prompt("Expansion (what gets typed):");
  if (!expansion) return;
  snippets.push({ trigger: trigger.trim(), expansion: expansion.trim() });
  await invoke("set_snippets", { snippets });
  await loadSnippets();
};

// Global hotkey drives recording.
listen("dictation:start", () => startRecording());
listen("dictation:stop", () => stopAndTranscribe());

// ---------- init ----------
(async function init() {
  await loadHistory();
  await loadStats();
  await loadDictionary();
  await loadSnippets();
  setState("idle", "Idle");
})();
