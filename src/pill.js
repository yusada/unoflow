// Floating pill window. Owns no state: it just reflects the events the main
// window (and Rust) already emit, so there's one source of truth.

const { listen } = window.__TAURI__.event;

const pill = document.getElementById("pill");
const label = document.getElementById("label");

const LABELS = {
  idle: "Idle",
  recording: "Listening…",
  processing: "Transcribing…",
  done: "Done",
};

function setState(state) {
  pill.dataset.state = state;
  label.textContent = LABELS[state] || state;
}

listen("pill:state", (e) => setState(e.payload));
