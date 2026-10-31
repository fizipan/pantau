// Interactions that work with or without scroll motion.
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const S = JSON.parse(document.body.dataset.strings);

// Language: remember the choice, so "/" doesn't send someone back.
$("#lang").addEventListener("click", (e) => { try { localStorage.setItem("pt-lang", e.currentTarget.dataset.lang); } catch {} });

// Screens in the hero: three real captures, switched by tabs.
const cap = $("#cap"), shots = $$(".shot"), tabs = $$(".tabs button");
function selectTab(tab, focus) {
  tabs.forEach((b) => { const on = b.dataset.tab === tab; b.setAttribute("aria-selected", on); b.tabIndex = on ? 0 : -1; if (on && focus) b.focus(); });
  shots.forEach((i) => { const on = i.dataset.tab === tab; i.classList.toggle("on", on); i.alt = on ? i.dataset.alt : ""; if (on) cap.textContent = i.dataset.cap; });
}
tabs.forEach((b, i) => {
  b.addEventListener("click", () => selectTab(b.dataset.tab));
  b.addEventListener("keydown", (e) => {
    const d = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : e.key === "Home" ? -99 : e.key === "End" ? 99 : 0;
    if (!d) return;
    e.preventDefault();
    const n = Math.abs(d) > 1 ? (d < 0 ? 0 : tabs.length - 1) : (i + d + tabs.length) % tabs.length;
    selectTab(tabs[n].dataset.tab, true);
  });
});
// Fetch the other screens once the page is idle, so the first click doesn't fade to nothing.
addEventListener("load", () => { const go = () => shots.forEach((i) => { i.loading = "eager"; }); window.requestIdleCallback ? requestIdleCallback(go) : setTimeout(go, 400); });

// Mobile menu
const menu = $("#menu"), navs = $("#navs");
const setMenu = (open) => { navs.classList.toggle("open", open); menu.setAttribute("aria-expanded", open); };
menu.addEventListener("click", () => setMenu(!navs.classList.contains("open")));
navs.addEventListener("click", (e) => { if (e.target.closest("a")) setMenu(false); });
document.addEventListener("click", (e) => { if (!e.target.closest("#menu, #navs")) setMenu(false); });
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && navs.classList.contains("open")) { setMenu(false); menu.focus(); } });

// Copy buttons
$$("[data-copy]").forEach((btn) => btn.addEventListener("click", async () => {
  const target = $(btn.dataset.copy);
  try {
    await navigator.clipboard.writeText(target.textContent);
    btn.textContent = S.copied; btn.dataset.done = "1";
    setTimeout(() => { btn.textContent = S.copy; delete btn.dataset.done; }, 1600);
  } catch {
    const r = document.createRange(); r.selectNodeContents(target);
    const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r);
  }
}));

// Reveal on scroll. Content is visible without this; it only plays the entrance.
const seen = new IntersectionObserver((list) => list.forEach((en) => { if (en.isIntersecting) { en.target.classList.add("in"); seen.unobserve(en.target); } }), { threshold: 0.15, rootMargin: "0px 0px -6% 0px" });
$$(".rise, .ladder").forEach((e) => seen.observe(e));
