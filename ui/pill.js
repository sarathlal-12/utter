const pill = document.getElementById("pill");
const msg = document.getElementById("msg");
const bars = [...pill.querySelectorAll(".bars i")];
const { listen } = window.__TAURI__.event;

let timer = null;
let idleVisible = true;

function setState(state, text) {
  clearTimeout(timer);
  pill.className = "pill " + state;
  if (state === "idle" && !idleVisible) pill.classList.add("hidden");
  msg.textContent = text || (state === "locked" ? "Hands-free · Ctrl+Win to stop" : "");
  if (state === "done") timer = setTimeout(() => setState("idle"), 700);
  if (state === "error") timer = setTimeout(() => setState("idle"), 2200);
  if (state !== "listening" && state !== "locked") bars.forEach((b) => (b.style.height = "4px"));
}

// Mic level -> bar heights. Centre bars react most, like a little waveform.
const shape = [0.45, 0.7, 0.9, 1, 0.9, 0.7, 0.45];
listen("level", (e) => {
  const v = Math.min(1, Math.sqrt(e.payload) * 3.2);
  bars.forEach((b, i) => {
    const jitter = 0.75 + Math.random() * 0.5;
    b.style.height = Math.max(4, Math.round(18 * v * shape[i] * jitter)) + "px";
  });
});

listen("pill", (e) => {
  idleVisible = e.payload.idle_visible;
  setState(e.payload.state, e.payload.msg);
});

setState("idle");
