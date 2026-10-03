import 'lenis/dist/lenis.css';
import './styles/base.css';
import './styles/sections.css';

import Lenis from 'lenis';
import { $, fontsReady, gsap, ScrollTrigger, still } from './lib/env';
import { initLanding } from './lib/land';
import { initApps } from './sections/apps';
import { initConfig, initDownload, initFaq, initModels, initPrivacy } from './sections/details';
import { initHero } from './sections/hero';
import { initReplay } from './sections/replay';
import { initSafety } from './sections/safety';

/** Smooths wheel scrolling so the scrubbed scenes move evenly. Off when motion is reduced. */
function initScroll(): void {
  // A phone's address bar sliding away resizes the viewport; that must not re-measure the page.
  ScrollTrigger.config({ ignoreMobileResize: true });
  if (still) return;
  const lenis = new Lenis({ lerp: 0.12, anchors: true });
  lenis.on('scroll', ScrollTrigger.update);
  gsap.ticker.add((time) => lenis.raf(time * 1000));
  gsap.ticker.lagSmoothing(0);
}

function initHeader(): void {
  const top = $('.top');
  const last = $('.cta');
  const update = () => {
    top.classList.toggle('is-stuck', scrollY > 16);
    // Over the closing section the header sits on the voice colour, so it takes that colour.
    top.classList.toggle('on-voice', last.getBoundingClientRect().top < top.offsetHeight / 2);
  };
  addEventListener('scroll', update, { passive: true });
  update();
}

initScroll();
initHeader();
initHero();
initLanding();
initApps();
initSafety();
initPrivacy();
initModels();
initConfig();
initFaq();
initDownload();

// The replay measures text, so it waits for the font.
void fontsReady(2500).then(initReplay);
