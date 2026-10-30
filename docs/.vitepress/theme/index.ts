import type { Theme } from 'vitepress'
import DefaultTheme from 'vitepress/theme'

import '@fontsource-variable/schibsted-grotesk/wght.css'
import '@fontsource-variable/schibsted-grotesk/wght-italic.css'
import '@fontsource-variable/public-sans/wght.css'
import '@fontsource-variable/public-sans/wght-italic.css'
import '@fontsource-variable/martian-mono/wdth.css'
import './style.css'

import Home from './components/Home.vue'
import Generations from './components/Generations.vue'
import Stages from './components/Stages.vue'

export default {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component('Home', Home)
    app.component('Generations', Generations)
    app.component('Stages', Stages)
  },
} satisfies Theme
