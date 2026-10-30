<script setup lang="ts">
import { computed, ref } from 'vue'

/*
 * The store, the generations and the pointer of a real machine: the
 * virtual machine test system before and after a service was added. The
 * store ids and the output lines come from that machine.
 */

const store = [
  { id: '7ah4dyc33f53wzrceczzpaspjwp5nhjx', name: 'runit-2.3.1+static' },
  { id: 'igyza4pekasw7tig4ana2lqhgotoy3ek', name: 'laft-0.1.0' },
  { id: 'k7pjia6tynb7vepkdzemiobizilxp7tl', name: 'busybox-1.37.0+static' },
  { id: 'y6jikgwolsdgntx6mkx5ard3ab3ymkdz', name: 'linux-7.2.13' },
  { id: 'yibf6gdfl34n22efd5uiz6obecmbacko', name: 'limine-11.2.1' },
]

const messages: Record<number, string> = {
  1: 'installed',
  2: 'add the hello service',
}

const gens = ref<number[]>([1])
const active = ref(1)
const flips = ref(0)
const output = ref<string[]>(['done        generation 1 is the default for the next boot'])

const stack = computed(() => [...gens.value].reverse())
const canApply = computed(() => !gens.value.includes(2))
const canRollback = computed(() => active.value === 2)
const canSwitch = computed(() => gens.value.includes(2) && active.value !== 2)

function activate(n: number, lines: string[]) {
  active.value = n
  flips.value += 1
  output.value = lines
}

function apply() {
  if (!canApply.value) return
  gens.value = [1, 2]
  activate(2, [
    'generation 2 (from 1)',
    '  services',
    '    + hello',
    '  download 0 B - store +0 B - 0 packages',
    'done        generation 2 is active and default',
  ])
}

function rollback() {
  if (!canRollback.value) return
  activate(1, [
    'activating  ok mount ok switch ok views ok etc ok accounts ok services ok unmount',
    'done        generation 1 is active and default',
  ])
}

function switchTo() {
  if (!canSwitch.value) return
  activate(2, ['done        generation 2 is active and default'])
}
</script>

<template>
  <figure class="lf-gens" aria-label="A Laft machine: its store, its generations and the active pointer">
    <div class="panes">
      <section class="pane store" aria-label="The store">
        <header>
          <span class="path">/laft/store</span>
          <span class="note">named by content hash</span>
        </header>
        <ul>
          <li v-for="e in store" :key="e.id">
            <span class="id">{{ e.id.slice(0, 8) }}</span><span class="rest">{{ e.id.slice(8) }}</span><span class="name">-{{ e.name }}</span>
            <span class="links" :title="`hardlinked into ${gens.length} generation${gens.length > 1 ? 's' : ''}`">
              <span v-for="g in gens" :key="g" class="link" :class="{ on: g === active }" />
            </span>
          </li>
        </ul>
      </section>

      <section class="pane gens" aria-label="The generations">
        <header>
          <span class="path">/laft/gens</span>
          <span class="note">numbered, never changed</span>
        </header>
        <TransitionGroup name="course" tag="ol">
          <li v-for="g in stack" :key="g" class="course" :class="{ active: g === active }">
            <span class="num">{{ g }}</span>
            <span class="flag" aria-hidden="true">{{ g === active ? '*' : '' }}</span>
            <span class="msg">{{ messages[g] }}</span>
            <span class="state">{{ g === active ? 'active, default' : 'kept' }}</span>
          </li>
        </TransitionGroup>
      </section>
    </div>

    <div class="pointer" aria-live="polite">
      <code :key="flips" class="flip">/run/laft/system -&gt; mounts/{{ active }}</code>
      <span class="why">one <code>rename(2)</code> switches every path below <code>/usr</code> and <code>/etc/static</code></span>
    </div>

    <div class="console">
      <div class="cmds" role="group" aria-label="Commands">
        <button type="button" :disabled="!canApply" @click="apply"><span class="p">$</span> laft apply</button>
        <button type="button" :disabled="!canRollback" @click="rollback"><span class="p">$</span> laft rollback</button>
        <button type="button" :disabled="!canSwitch" @click="switchTo"><span class="p">$</span> laft switch 2</button>
      </div>
      <pre class="out" aria-live="polite"><template v-for="(l, i) in output" :key="`${flips}-${i}`">{{ l }}
</template></pre>
    </div>
    <figcaption>
      Try the commands. The store ids and output lines come from the test machine, where generation 2 added a
      service and reused all five store entries.
    </figcaption>
  </figure>
</template>

<style scoped>
.lf-gens {
  margin: 0;
  border: 1px solid var(--vp-c-border);
  border-radius: 10px;
  background: var(--vp-c-bg-elv);
  box-shadow:
    0 1px 0 rgba(0, 0, 0, 0.02),
    0 18px 40px -24px rgba(12, 30, 38, 0.35);
  overflow: hidden;
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
}

.panes {
  display: grid;
  grid-template-columns: minmax(0, 1.25fr) minmax(0, 1fr);
}

.pane {
  padding: 16px 18px 18px;
  min-width: 0;
}

.pane + .pane {
  border-left: 1px solid var(--vp-c-divider);
}

header {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  align-items: baseline;
  gap: 2px 8px;
  margin-bottom: 12px;
}

.path {
  font-size: 12.5px;
  font-weight: 650;
  color: var(--vp-c-text-1);
}

.note {
  font-size: 10.5px;
  letter-spacing: 0.04em;
  color: var(--vp-c-text-3);
  white-space: nowrap;
}

ul,
ol {
  list-style: none;
  margin: 0;
  padding: 0;
}

.store li {
  display: flex;
  align-items: center;
  gap: 0;
  padding: 5px 0;
  font-size: 11.5px;
  border-bottom: 1px dashed var(--vp-c-divider);
  white-space: nowrap;
}

.store li:last-child {
  border-bottom: none;
}

.id {
  color: var(--vp-c-brand-1);
  font-weight: 600;
}

.rest {
  color: var(--vp-c-text-3);
  overflow: hidden;
  text-overflow: clip;
  max-width: 7ch;
}

.rest::after {
  content: '…';
}

.name {
  color: var(--vp-c-text-1);
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
}

.links {
  display: inline-flex;
  gap: 3px;
  margin-left: 8px;
}

.link {
  width: 12px;
  height: 6px;
  border-radius: 3px;
  background: var(--vp-c-text-3);
  opacity: 0.55;
}

.link.on {
  background: var(--lf-lichen-bright);
  opacity: 1;
}

.gens ol {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.course {
  display: grid;
  grid-template-columns: 2.2ch 1.4ch minmax(0, 1fr);
  grid-template-rows: auto auto;
  column-gap: 6px;
  align-items: center;
  padding: 9px 12px;
  border-radius: 999px;
  background: var(--vp-c-bg-alt);
  border: 1px solid var(--vp-c-divider);
  font-size: 11.5px;
  transition: background-color 0.3s, border-color 0.3s;
}

.course .num {
  grid-row: 1 / 3;
  font-size: 15px;
  font-weight: 700;
  color: var(--vp-c-text-2);
}

.course .flag {
  grid-row: 1 / 3;
  font-weight: 800;
  color: var(--lf-lichen);
}

.course .msg {
  color: var(--vp-c-text-1);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.course .state {
  grid-column: 3;
  font-size: 10px;
  letter-spacing: 0.05em;
  color: var(--vp-c-text-3);
}

.course.active {
  background: color-mix(in srgb, var(--lf-lichen-bright) 16%, var(--vp-c-bg-elv));
  border-color: color-mix(in srgb, var(--lf-lichen-bright) 70%, transparent);
}

.course.active .num {
  color: var(--vp-c-text-1);
}

.course-enter-active {
  transition: transform 0.45s cubic-bezier(0.2, 0.8, 0.2, 1), opacity 0.45s;
}

.course-enter-from {
  transform: translateY(-14px);
  opacity: 0;
}

.course-move {
  transition: transform 0.45s;
}

.pointer {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 6px 14px;
  padding: 12px 18px;
  border-top: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-alt);
}

.pointer .flip {
  font-size: 12.5px;
  font-weight: 650;
  color: var(--vp-c-text-1);
  animation: flip 0.6s ease-out;
}

@keyframes flip {
  from {
    background: color-mix(in srgb, var(--lf-lichen-bright) 45%, transparent);
  }
  to {
    background: transparent;
  }
}

.pointer .why {
  font-family: var(--vp-font-family-base);
  font-stretch: 100%;
  font-size: 12.5px;
  color: var(--vp-c-text-2);
}

.pointer .why code {
  font-family: var(--vp-font-family-mono);
  font-stretch: 87.5%;
  font-size: 11.5px;
}

.console {
  background: #0f191e;
  color: #d4dfe3;
  padding: 14px 18px 16px;
}

.cmds {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 12px;
}

.cmds button {
  padding: 6px 12px;
  border: 1px solid #2c3d45;
  border-radius: 6px;
  background: #15232a;
  color: #e2eaed;
  font-family: inherit;
  font-stretch: inherit;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: border-color 0.2s, background-color 0.2s, transform 0.2s;
}

.cmds button:hover:not(:disabled) {
  border-color: #5fd1c8;
  transform: translateY(-1px);
}

.cmds button:disabled {
  opacity: 0.38;
  cursor: not-allowed;
}

.cmds .p {
  color: #b5d63c;
}

.out {
  margin: 0;
  min-height: 7.6em;
  font-family: inherit;
  font-size: 11.5px;
  line-height: 1.6;
  white-space: pre-wrap;
  color: #c9d6db;
}

figcaption {
  padding: 10px 18px 12px;
  border-top: 1px solid var(--vp-c-divider);
  font-family: var(--vp-font-family-base);
  font-stretch: 100%;
  font-size: 12.5px;
  color: var(--vp-c-text-3);
}

@media (max-width: 720px) {
  .panes {
    grid-template-columns: 1fr;
  }

  .pane + .pane {
    border-left: none;
    border-top: 1px solid var(--vp-c-divider);
  }
}
</style>
