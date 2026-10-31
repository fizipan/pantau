// Scroll motion. Everything is visible without this file, and it never runs under reduced motion.
import gsap from "gsap";
import { ScrollTrigger } from "gsap/ScrollTrigger";
import { SplitText } from "gsap/SplitText";
import Lenis from "lenis";

const root = document.documentElement;

if (root.classList.contains("motion")) {
  gsap.registerPlugin(ScrollTrigger, SplitText);
  const lenis = new Lenis({ lerp: 0.12, smoothWheel: true, syncTouch: false, anchors: true });
  lenis.on("scroll", ScrollTrigger.update);
  gsap.ticker.add((t) => lenis.raf(t * 1000));
  gsap.ticker.lagSmoothing(0);
  document.querySelector(".to-top")?.addEventListener("click", (e) => { e.preventDefault(); lenis.scrollTo(0); });

  const start = async () => {
    await Promise.race([document.fonts.ready, new Promise((r) => setTimeout(r, 2500))]);
    root.classList.add("ready");

    // The one authored moment: the dial from the icon turns and drifts as the first screen leaves.
    const mm = gsap.matchMedia();
    mm.add("(min-width: 900px)", () => {
      gsap.to("#gauge", { rotate: 38, y: -60, ease: "none", scrollTrigger: { trigger: ".hero", start: "top top", end: "bottom top", scrub: 0.6 } });
    });

    // Section titles come up line by line.
    gsap.utils.toArray("[data-title]").forEach((el) => {
      const split = SplitText.create(el, { type: "lines", mask: "lines" });
      gsap.from(split.lines, { yPercent: 110, duration: 0.9, ease: "power4.out", stagger: 0.08, scrollTrigger: { trigger: el, start: "top 88%" }, onComplete: () => split.revert() });
    });
    ScrollTrigger.refresh();
  };
  start();
}
