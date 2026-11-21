const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let cfg = null; // { settings, recent_apps, version, hardware }
let history = [];
let saveTimer = null;

// ---------------------------------------------------------------- helpers

function el(tag, attrs = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v);
  }
  for (const k of kids) e.append(k);
  return e;
}

function flashSaved(text = "Saved") {
  const s = $("saved");
  s.textContent = text;
  s.classList.add("show");
  setTimeout(() => s.classList.remove("show"), 1400);
}

function save() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try {
      await invoke("save_config", { settings: cfg.settings });
      flashSaved();
    } catch (e) {
      flashSaved("Couldn't save: " + e);
    }
  }, 250);
}

// ---------------------------------------------------------------- tabs

function showTab(name) {
  document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t.id === "tab-" + name));
  document.querySelectorAll("#tabs button").forEach((b) => b.classList.toggle("active", b.dataset.tab === name));
  if (name === "welcome") document.querySelector('[data-tab="welcome"]').hidden = false;
  if (name === "history") loadHistory();
}
document.querySelectorAll("#tabs button").forEach((b) => b.addEventListener("click", () => showTab(b.dataset.tab)));
document.querySelectorAll("[data-goto]").forEach((b) => b.addEventListener("click", () => showTab(b.dataset.goto)));
listen("open-tab", (e) => showTab(e.payload));

// ---------------------------------------------------------------- general

const toggles = ["fast_mode", "ai_cleanup", "show_idle_pill", "autostart"];
toggles.forEach((id) =>
  $(id).addEventListener("change", (e) => {
    cfg.settings[id] = e.target.checked;
    $("ai_cleanup").disabled = cfg.settings.fast_mode;
    save();
  })
);

$("hk-change").addEventListener("click", async () => {
  $("capture").hidden = false;
  $("capture-keys").textContent = "…";
  const combo = await invoke("capture_hotkey");
  $("capture").hidden = true;
  if (!combo) return;
  if (!/(Ctrl|Alt|Shift|Win)/.test(combo) && !/^F\d+$/.test(combo)) {
    alert(`"${combo}" on its own would block normal typing. Use at least one of Ctrl, Alt, Shift or Win, or a function key (F1–F24).`);
    return;
  }
  cfg.settings.hotkey = combo;
  renderGeneral();
  save();
});

function renderGeneral() {
  const s = cfg.settings;
  toggles.forEach((id) => ($(id).checked = s[id]));
  $("ai_cleanup").disabled = s.fast_mode;
  $("hotkey").textContent = s.hotkey;
  document.querySelectorAll(".hk").forEach((k) => (k.textContent = s.hotkey));
  const hw = cfg.hardware;
  $("meta").textContent = `Utter ${cfg.version} · ${hw.ram_gb} GB RAM · ${hw.threads} threads`;
  const tierText = { low: "a lighter computer, so Fast Mode is on", mid: "a mid-range computer, so AI cleanup is off to keep things quick", high: "a capable computer, so AI cleanup is on" };
  $("hw-card").textContent = `Utter checked this PC (${hw.ram_gb} GB RAM, ${hw.threads} threads) and set it up for ${tierText[s.tier] || "your hardware"}. You can change this in Settings anytime.`;
}

// ---------------------------------------------------------------- dictionary

function renderWords() {
  const box = $("words");
  box.replaceChildren();
  if (!cfg.settings.words.length) box.append(el("div", { class: "empty" }, "No words yet."));
  cfg.settings.words.forEach((w, i) =>
    box.append(
      el("span", { class: "chip" }, w, el("button", { title: "Remove", onclick: () => { cfg.settings.words.splice(i, 1); renderWords(); save(); } }, "×"))
    )
  );
}
$("word-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const words = $("word-input").value.split(",").map((w) => w.trim()).filter(Boolean);
  for (const w of words) if (!cfg.settings.words.some((x) => x.toLowerCase() === w.toLowerCase())) cfg.settings.words.push(w);
  $("word-input").value = "";
  renderWords();
  save();
});

function renderReps() {
  const box = $("reps");
  box.replaceChildren();
  if (!cfg.settings.replacements.length) box.append(el("div", { class: "empty" }, "No replacements yet."));
  cfg.settings.replacements.forEach((r, i) =>
    box.append(
      el("div", { class: "item" },
        el("div", { class: "grow" }, el("span", {}, r.from), el("span", { class: "arrow" }, "  →  "), el("b", {}, r.to)),
        el("div", { class: "actions" }, el("button", { class: "small danger", onclick: () => { cfg.settings.replacements.splice(i, 1); renderReps(); save(); } }, "Delete"))
      )
    )
  );
}
$("rep-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const from = $("rep-from").value.trim(), to = $("rep-to").value.trim();
  if (!from || !to) return;
  cfg.settings.replacements.push({ from, to });
  $("rep-from").value = $("rep-to").value = "";
  renderReps();
  save();
});

// ---------------------------------------------------------------- snippets

function renderSnips() {
  const box = $("snips");
  box.replaceChildren();
  if (!cfg.settings.snippets.length) box.append(el("div", { class: "empty" }, "No snippets yet. Try one for your email address or a standard reply."));
  cfg.settings.snippets.forEach((s, i) =>
    box.append(
      el("div", { class: "item" },
        el("div", { class: "grow" }, el("div", { class: "sub" }, `Say “${s.trigger}”`), el("div", { class: "text" }, s.text)),
        el("div", { class: "actions" }, el("button", { class: "small danger", onclick: () => { cfg.settings.snippets.splice(i, 1); renderSnips(); save(); } }, "Delete"))
      )
    )
  );
}
$("snip-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const trigger = $("snip-trigger").value.trim(), text = $("snip-text").value;
  if (!trigger || !text.trim()) return;
  cfg.settings.snippets = cfg.settings.snippets.filter((s) => s.trigger.toLowerCase() !== trigger.toLowerCase());
  cfg.settings.snippets.push({ trigger, text });
  $("snip-trigger").value = $("snip-text").value = "";
  renderSnips();
  save();
});

// ---------------------------------------------------------------- app tones

const toneNames = { casual: "Casual", formal: "Formal", neutral: "Neutral", code: "Code (no changes)" };
function renderTones() {
  const box = $("tones");
  box.replaceChildren();
  if (!cfg.settings.app_tones.length) box.append(el("div", { class: "empty" }, "No custom rules. The built-in ones are in use."));
  cfg.settings.app_tones.forEach((t, i) =>
    box.append(
      el("div", { class: "item" },
        el("div", { class: "grow" }, el("b", {}, t.app), el("span", { class: "arrow" }, "  →  "), toneNames[t.tone] || t.tone),
        el("div", { class: "actions" }, el("button", { class: "small danger", onclick: () => { cfg.settings.app_tones.splice(i, 1); renderTones(); save(); } }, "Delete"))
      )
    )
  );
  const dl = $("recent-apps");
  dl.replaceChildren(...cfg.recent_apps.map((a) => el("option", { value: a })));
}
$("tone-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const app = $("tone-app").value.trim();
  if (!app) return;
  cfg.settings.app_tones = cfg.settings.app_tones.filter((t) => t.app.toLowerCase() !== app.toLowerCase());
  cfg.settings.app_tones.push({ app, tone: $("tone-pick").value });
  $("tone-app").value = "";
  renderTones();
  save();
});

// ---------------------------------------------------------------- history

async function loadHistory() {
  history = await invoke("get_history");
  renderHistory();
}

function dayLabel(d) {
  const today = new Date();
  const y = new Date(today);
  y.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === y.toDateString()) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
}

function renderHistory() {
  const box = $("history");
  box.replaceChildren();
  const q = $("search").value.trim().toLowerCase();
  const showRaw = $("show-raw").checked;
  const rows = history.filter((h) => !q || h.text.toLowerCase().includes(q) || h.raw.toLowerCase().includes(q) || h.app.toLowerCase().includes(q));
  if (!rows.length) {
    box.append(el("div", { class: "empty" }, history.length ? "Nothing matches your search." : "Your dictations will show up here."));
    return;
  }
  let lastDay = "";
  for (const h of rows) {
    const d = new Date(h.ts * 1000);
    const day = dayLabel(d);
    if (day !== lastDay) {
      box.append(el("div", { class: "day" }, day));
      lastDay = day;
    }
    const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    const app = h.app ? h.app.replace(/\.exe$/i, "") : "unknown app";
    const body = el("div", { class: "grow" }, el("div", { class: "sub" }, `${time} · ${app} · ${h.secs.toFixed(1)} s`), el("div", { class: "text" }, h.text));
    if (showRaw && h.raw !== h.text) body.append(el("div", { class: "raw" }, h.raw));
    box.append(
      el("div", { class: "item" },
        body,
        el("div", { class: "actions" },
          el("button", { class: "small", title: "Copy to clipboard", onclick: async () => { await invoke("copy_text", { text: h.text }); flashSaved("Copied"); } }, "Copy"),
          el("button", { class: "small", title: "Paste into the app you were using", onclick: () => invoke("paste_text", { text: h.text }) }, "Paste"),
          el("button", { class: "small danger", title: "Delete", onclick: async () => { await invoke("delete_history", { id: h.id }); loadHistory(); } }, "×")
        )
      )
    );
  }
}
$("search").addEventListener("input", renderHistory);
$("show-raw").addEventListener("change", renderHistory);
$("clear").addEventListener("click", async () => {
  if (!history.length) return;
  if (!confirm(`Delete all ${history.length} dictations? This can't be undone.`)) return;
  await invoke("clear_history");
  loadHistory();
});
listen("history-changed", () => {
  if ($("tab-history").classList.contains("active")) loadHistory();
});

// ---------------------------------------------------------------- boot

async function loadConfig() {
  cfg = await invoke("get_config");
  renderGeneral();
  renderWords();
  renderReps();
  renderSnips();
  renderTones();
}
// The tray can change settings while this window is open.
listen("config-changed", async () => {
  const fresh = await invoke("get_config");
  cfg.recent_apps = fresh.recent_apps;
  for (const id of toggles) cfg.settings[id] = fresh.settings[id];
  renderGeneral();
});

loadConfig().then(() => showTab(window.__UTTER_TAB || "general"));
