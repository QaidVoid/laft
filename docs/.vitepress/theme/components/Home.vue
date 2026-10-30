<script setup lang="ts">
import { withBase } from 'vitepress'

import Generations from './Generations.vue'

const ideas = [
  {
    name: 'One declarative file',
    body: 'Packages, users, /etc, filesystems, the boot loader and services are described in Stav, a typed configuration language. The whole file is checked before anything changes.',
    code: 'laft check',
    link: '/guide/configuration',
  },
  {
    name: 'A content addressed store',
    body: 'Every package unpacks once into /laft/store, named by the BLAKE3 hash of its manifest. Entries are immutable, so many versions of a package can sit side by side.',
    code: '/laft/store/7ah4dyc3…-runit-2.3.1+static',
    link: '/guide/concepts#the-store',
  },
  {
    name: 'Views made of hardlinks',
    body: 'Each /usr is a complete tree assembled from store entries with hardlinks. Nothing is copied, and the system view is just the view that /usr points at.',
    code: '/usr -> /run/laft/system/usr',
    link: '/guide/concepts#views',
  },
  {
    name: 'Generations and rollback',
    body: 'Applying the configuration builds a new numbered generation and switches to it with one rename. The previous one stays whole, in the boot menu and one command away.',
    code: 'laft rollback',
    link: '/guide/everyday#rolling-back',
  },
  {
    name: 'Services for runit',
    body: 'Services are declared once, with dependencies and readiness checks, and rendered for runit. Between generations, Laft works out which services to start, stop or restart.',
    code: 'service sshd { after = [sshd-keygen] }',
    link: '/guide/concepts#services',
  },
  {
    name: 'Hermetic builds from source',
    body: 'Every package is built from pinned sources in a sandbox with no network, bootstrapped from a cross toolchain, and checked to rebuild byte for byte.',
    code: 'laft-build check-repro',
    link: '/guide/building',
  },
]

const flow = [
  { name: 'Recipe', tool: 'package.stav', body: 'Pinned sources, a build style and declared dependencies.' },
  { name: 'Build', tool: 'laft-build', body: 'A sandbox with a view of the build store as /usr and no network.' },
  { name: 'Package', tool: '.lpkg', body: 'A reproducible archive whose store id is the hash of its manifest.' },
  { name: 'Publish', tool: 'laft-repo', body: 'A minisign-signed index whose generation never goes back.' },
  { name: 'Apply', tool: 'laft apply', body: 'Resolve, assemble and switch to a new generation in one rename.' },
  { name: 'Boot', tool: 'laft-boot', body: 'The initramfs picks a generation, counts the attempt and starts runit.' },
]

const works = [
  'Install onto a disk image and boot it under UEFI with Limine',
  'Apply, roll back, switch and garbage collect generations',
  'Boot counting with fallback to the last good generation',
  'A base system of 89 recipes, bootstrapped and rebuilt hermetically',
]

const planned = [
  'Package options and prebuilt profiles',
  'Static musl variants of packages',
  'Hosted, signed package repositories',
  'Installs on real hardware, and a desktop',
]
</script>

<template>
  <div class="lf-home">
    <section class="hero">
      <div class="hero-text">
        <div class="lockup" aria-hidden="true">
          <svg class="mark" viewBox="0 0 32 32">
            <rect class="top" x="4" y="3" width="10.5" height="5.5" rx="2.75" />
            <rect x="4" y="9.75" width="10.5" height="5.5" rx="2.75" />
            <rect x="4" y="16.5" width="10.5" height="5.5" rx="2.75" />
            <rect x="4" y="23.25" width="24" height="5.5" rx="2.75" />
          </svg>
          <span class="word">laft</span>
        </div>
        <p class="eyebrow">A Linux distribution <span class="dot" aria-hidden="true" /> pre-release</p>
        <h1>
          One file describes the whole system.
          <span class="accent">Every change can be rolled back.</span>
        </h1>
        <p class="lead">
          Laft is a Linux distribution built from source, with all of its tooling written in Rust. Packages live in a
          content addressed store, and applying the configuration builds a new, immutable generation of the system. The
          machine switches to it in one atomic step and keeps the previous one to go back to.
        </p>
        <div class="actions">
          <a class="btn brand" :href="withBase('/guide/try-it')">Try it in QEMU</a>
          <a class="btn alt" :href="withBase('/guide/concepts')">How it works</a>
          <a class="btn ghost" href="https://github.com/QaidVoid/laft">Source</a>
        </div>
      </div>
      <div class="hero-art">
        <Generations />
      </div>
    </section>

    <section class="ideas" aria-labelledby="lf-ideas">
      <h2 id="lf-ideas">What makes Laft different</h2>
      <p class="section-lead">
        Six ideas shape the whole design. Laft's tools enforce each one, so they hold on every machine instead of
        depending on convention.
      </p>
      <ol class="grid">
        <li v-for="(idea, i) in ideas" :key="idea.name" class="idea">
          <span class="idx" aria-hidden="true">0{{ i + 1 }}</span>
          <h3>
            <a :href="withBase(idea.link)">{{ idea.name }}</a>
          </h3>
          <p>{{ idea.body }}</p>
          <code>{{ idea.code }}</code>
        </li>
      </ol>
    </section>

    <section class="config" aria-labelledby="lf-config">
      <div class="config-text">
        <h2 id="lf-config">From a configuration file to a bootable system</h2>
        <p class="section-lead">
          This is part of the configuration that the virtual machine test installs. <code>laft install</code> reads it,
          resolves five packages from a signed repository and assembles generation 1, with its services and boot entry.
        </p>
        <a class="more" :href="withBase('/guide/configuration')">Configuring a system →</a>
      </div>
      <div class="bench">
        <div class="bench-pane">
          <div class="bench-bar"><span class="tab">/etc/laft/system.stav</span></div>
          <div class="bench-code vp-doc"><slot name="config" /></div>
        </div>
        <div class="bench-pane">
          <div class="bench-bar"><span class="tab"><span class="p">#</span> laft install</span></div>
          <div class="bench-code vp-doc"><slot name="plan" /></div>
        </div>
      </div>
    </section>

    <section class="flow" aria-labelledby="lf-flow">
      <h2 id="lf-flow">From recipe to running machine</h2>
      <p class="section-lead">
        Every tool along the way is a Laft program, written in Rust and linked statically, so it runs even with an
        empty <code>/usr</code>.
      </p>
      <ol>
        <li v-for="(f, i) in flow" :key="f.name">
          <span class="n" aria-hidden="true">{{ i + 1 }}</span>
          <h3>{{ f.name }}</h3>
          <code>{{ f.tool }}</code>
          <p>{{ f.body }}</p>
        </li>
      </ol>
    </section>

    <section class="status" aria-labelledby="lf-status">
      <div class="status-head">
        <h2 id="lf-status">Where Laft stands today</h2>
        <p>
          Laft boots in a virtual machine today, and its test suite installs and boots it end to end. It is not ready
          for real hardware or daily use yet.
        </p>
        <a class="more" :href="withBase('/guide/status')">Status and roadmap →</a>
      </div>
      <div class="lists">
        <div>
          <h3><span class="works">works</span></h3>
          <ul>
            <li v-for="w in works" :key="w">{{ w }}</li>
          </ul>
        </div>
        <div>
          <h3><span class="planned">planned</span></h3>
          <ul>
            <li v-for="p in planned" :key="p">{{ p }}</li>
          </ul>
        </div>
      </div>
    </section>

    <section class="closing">
      <h2>Boot Laft in a virtual machine</h2>
      <p>One script builds the packages, installs a system onto a disk image and boots it under QEMU. Keep the image, and it is yours to explore.</p>
      <div class="actions center">
        <a class="btn brand" :href="withBase('/guide/try-it')">Try it in QEMU</a>
        <a class="btn ghost" :href="withBase('/guide/introduction')">Read the guide</a>
      </div>
    </section>
  </div>
</template>

<style scoped>
.lf-home {
  max-width: 1240px;
  margin: 0 auto;
  padding: 0 24px 96px;
}

h1,
h2,
h3,
.btn,
.word {
  font-family: var(--lf-font-display);
}

h2 {
  margin: 0 0 12px;
  font-size: clamp(28px, 3.4vw, 38px);
  font-weight: 760;
  letter-spacing: -0.03em;
  line-height: 1.12;
}

.section-lead {
  max-width: 640px;
  margin: 0 0 36px;
  font-size: 17px;
  line-height: 1.65;
  color: var(--vp-c-text-2);
}

.section-lead code,
.lead code {
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 0.84em;
  padding: 2px 5px;
  border-radius: 3px;
  background: var(--vp-code-bg);
  color: var(--vp-code-color);
}

/* -------------------------------------------------------------- hero --- */

.hero {
  display: grid;
  grid-template-columns: minmax(0, 0.95fr) minmax(0, 1.05fr);
  gap: 56px;
  align-items: center;
  padding: 56px 0 88px;
}

.lockup {
  display: inline-flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 28px;
}

.mark {
  width: 52px;
  height: 52px;
}

.mark rect {
  fill: var(--lf-log);
}

.mark .top {
  fill: var(--vp-c-brand-3);
}

.word {
  font-size: 46px;
  font-weight: 800;
  letter-spacing: -0.05em;
  line-height: 1;
  color: var(--vp-c-text-1);
  transform: translateY(2px);
}

.eyebrow {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 0 0 18px;
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--vp-c-text-2);
}

.eyebrow .dot {
  width: 12px;
  height: 6px;
  border-radius: 3px;
  background: var(--lf-lichen-bright);
}

h1 {
  margin: 0;
  font-size: clamp(36px, 4.6vw, 56px);
  font-weight: 800;
  line-height: 1.04;
  letter-spacing: -0.04em;
  color: var(--vp-c-text-1);
}

.accent {
  display: block;
  color: var(--vp-c-brand-1);
}

.lead {
  max-width: 560px;
  margin: 24px 0 0;
  font-size: 18px;
  line-height: 1.65;
  color: var(--vp-c-text-2);
}

.actions {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  margin-top: 32px;
}

.actions.center {
  justify-content: center;
}

.btn {
  display: inline-flex;
  align-items: center;
  height: 46px;
  padding: 0 22px;
  border: 1.5px solid transparent;
  border-radius: 999px;
  font-size: 15.5px;
  font-weight: 680;
  text-decoration: none;
  transition: transform 0.18s, background-color 0.18s, border-color 0.18s;
}

.btn:hover {
  transform: translateY(-2px);
}

.btn.brand {
  background: var(--vp-button-brand-bg);
  color: var(--vp-button-brand-text);
}

.btn.brand:hover {
  background: var(--vp-button-brand-hover-bg);
}

.btn.alt {
  background: var(--vp-c-text-1);
  color: var(--vp-c-bg);
}

.btn.ghost {
  border-color: var(--vp-c-border);
  color: var(--vp-c-text-1);
}

.btn.ghost:hover {
  border-color: var(--vp-c-brand-2);
}

/* ------------------------------------------------------------- ideas --- */

.ideas {
  padding: 24px 0 88px;
}

.grid {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 0;
  border-top: 1px solid var(--vp-c-border);
}

.idea {
  position: relative;
  padding: 26px 26px 28px 0;
  border-bottom: 1px solid var(--vp-c-divider);
  display: flex;
  flex-direction: column;
}

.idea:not(:nth-child(3n + 1)) {
  padding-left: 26px;
  border-left: 1px solid var(--vp-c-divider);
}

.idx {
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 11px;
  font-weight: 600;
  color: var(--vp-c-text-3);
  letter-spacing: 0.08em;
}

.idea h3 {
  margin: 10px 0 8px;
  font-size: 20px;
  font-weight: 720;
  letter-spacing: -0.02em;
}

.idea h3 a {
  color: var(--vp-c-text-1);
  text-decoration: none;
}

.idea h3 a:hover {
  color: var(--vp-c-brand-1);
}

.idea p {
  margin: 0 0 16px;
  font-size: 15px;
  line-height: 1.65;
  color: var(--vp-c-text-2);
  flex: 1;
}

.idea code {
  align-self: flex-start;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  padding: 4px 10px;
  border-radius: 999px;
  background: var(--vp-c-bg-alt);
  border: 1px solid var(--vp-c-divider);
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 11.5px;
  color: var(--vp-c-text-1);
}

/* ------------------------------------------------------------ config --- */

.config {
  display: grid;
  grid-template-columns: minmax(0, 0.7fr) minmax(0, 1.3fr);
  gap: 48px;
  padding: 0 0 96px;
  align-items: start;
}

.config-text {
  position: sticky;
  top: calc(var(--vp-nav-height) + 32px);
}

.more {
  font-family: var(--lf-font-display);
  font-weight: 650;
  color: var(--vp-c-brand-1);
  text-decoration: none;
}

.more:hover {
  text-decoration: underline;
  text-underline-offset: 3px;
}

.bench {
  display: grid;
  gap: 14px;
}

.bench-pane {
  border: 1px solid var(--vp-c-border);
  border-radius: 10px;
  overflow: hidden;
  background: var(--vp-code-block-bg);
}

.bench-bar {
  display: flex;
  align-items: center;
  padding: 9px 16px;
  border-bottom: 1px solid var(--vp-code-block-divider-color);
  background: var(--vp-c-bg-alt);
}

.tab {
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 12px;
  font-weight: 600;
  color: var(--vp-c-text-2);
}

.tab .p {
  color: var(--lf-lichen);
}

.bench-code :deep(div[class*='language-']) {
  margin: 0 !important;
  border: none !important;
  border-radius: 0 !important;
}

.bench-code :deep(span.lang),
.bench-code :deep(button.copy) {
  display: none;
}

/* -------------------------------------------------------------- flow --- */

.flow {
  padding: 0 0 96px;
}

.flow ol {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  grid-template-columns: repeat(6, minmax(0, 1fr));
  gap: 12px;
}

.flow li {
  position: relative;
  padding: 16px 16px 18px;
  border-radius: 10px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-elv);
}

.flow li:not(:last-child)::after {
  content: '';
  position: absolute;
  top: 28px;
  right: -13px;
  width: 14px;
  height: 2px;
  background: var(--vp-c-border);
}

.flow .n {
  display: inline-grid;
  place-items: center;
  width: 24px;
  height: 24px;
  border-radius: 999px;
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
  font-weight: 700;
}

.flow h3 {
  margin: 12px 0 4px;
  font-size: 17px;
  font-weight: 720;
}

.flow code {
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 11px;
  color: var(--vp-c-brand-1);
}

.flow p {
  margin: 8px 0 0;
  font-size: 13.5px;
  line-height: 1.55;
  color: var(--vp-c-text-2);
}

/* ------------------------------------------------------------ status --- */

.status {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr);
  gap: 48px;
  padding: 40px;
  margin-bottom: 96px;
  border-radius: 14px;
  border: 1px solid var(--vp-c-border);
  background:
    repeating-linear-gradient(180deg, transparent 0 23px, var(--lf-course) 23px 24px),
    var(--vp-c-bg-alt);
}

.status-head p {
  margin: 0 0 18px;
  font-size: 16.5px;
  line-height: 1.65;
  color: var(--vp-c-text-2);
}

.lists {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 28px;
}

.lists h3 {
  margin: 4px 0 12px;
}

.lists ul {
  margin: 0;
  padding: 0;
  list-style: none;
}

.lists li {
  position: relative;
  padding: 6px 0 6px 20px;
  font-size: 14.5px;
  line-height: 1.5;
  color: var(--vp-c-text-1);
}

.lists li::before {
  content: '';
  position: absolute;
  left: 0;
  top: 13px;
  width: 10px;
  height: 5px;
  border-radius: 3px;
  background: var(--vp-c-brand-3);
}

.lists > div:last-child li::before {
  background: transparent;
  border: 1px dashed var(--vp-c-text-3);
}

.works,
.planned {
  display: inline-block;
  padding: 2px 9px;
  border-radius: 4px;
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.works {
  border: 1px solid var(--vp-c-brand-3);
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.planned {
  border: 1px dashed var(--vp-c-text-3);
  color: var(--vp-c-text-2);
}

/* ----------------------------------------------------------- closing --- */

.closing {
  text-align: center;
}

.closing p {
  max-width: 560px;
  margin: 0 auto;
  font-size: 17px;
  line-height: 1.65;
  color: var(--vp-c-text-2);
}

/* ------------------------------------------------------------ narrow --- */

@media (max-width: 1100px) {
  .flow ol {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }

  .flow li:nth-child(3n)::after {
    display: none;
  }
}

@media (max-width: 960px) {
  .hero,
  .config,
  .status {
    grid-template-columns: minmax(0, 1fr);
  }

  .hero {
    gap: 40px;
    padding-top: 32px;
  }

  .config-text {
    position: static;
  }

  .grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .idea:not(:nth-child(3n + 1)) {
    padding-left: 0;
    border-left: none;
  }

  .idea:nth-child(2n) {
    padding-left: 22px;
    border-left: 1px solid var(--vp-c-divider);
  }
}

@media (max-width: 640px) {
  .lf-home {
    padding: 0 16px 72px;
  }

  .mark {
    width: 40px;
    height: 40px;
  }

  .word {
    font-size: 36px;
  }

  .grid,
  .lists {
    grid-template-columns: minmax(0, 1fr);
  }

  .flow ol {
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 10px;
  }

  .flow li {
    padding: 14px 14px 16px;
  }

  .idea:nth-child(2n) {
    padding-left: 0;
    border-left: none;
  }

  .flow li::after {
    display: none;
  }

  .status {
    padding: 24px 20px;
  }

  .bench-pane {
    border-radius: 8px;
  }
}
</style>
