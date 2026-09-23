<script setup lang="ts">
// The realtime "press / to search this repo's files" box — the one piece
// of client-side interactivity in this UI. In the old Leptos version this
// needed its own WASM crate, an `#[island]`, and a hand-vendored
// hydration driver script; here it's just a normal Vue component, because
// this is exactly Vue's home turf.

const props = defineProps<{
  repo: string
  gitRef: string
}>()

const open = ref(false)
const query = ref('')
const files = ref<string[]>([])
const inputEl = ref<HTMLInputElement | null>(null)

async function loadFiles() {
  try {
    const data = await graphqlRequest<{ repository: { files: string[] } | null }>(
      `query($name: String!, $ref: String!) { repository(name: $name) { files(ref: $ref) } }`,
      { name: props.repo, ref: props.gitRef }
    )
    files.value = data.repository?.files ?? []
  } catch {
    files.value = []
  }
}

async function openAndFocus() {
  open.value = true
  await nextTick()
  // nextTick only waits for Vue's own render flush (a microtask) — it
  // doesn't guarantee the browser has finished a style/layout pass, so
  // the input can still compute as `display:none` at this point. Chrome
  // tolerates focusing it anyway; Firefox silently no-ops the focus()
  // call instead (the exact same class of bug the old Leptos version hit
  // here, fixed the same way: defer one more tick, to after layout).
  requestAnimationFrame(() => inputEl.value?.focus())
  if (!files.value.length) {
    await loadFiles()
  }
}

// Firefox's built-in Quick Find also binds "/" — it's a browser-chrome
// command, not a page-level default action, so calling preventDefault()
// on `keydown` alone doesn't reliably suppress it in every Firefox
// version. Capturing the event (running before it reaches other
// listeners) and preventing default on both `keydown` and `keypress`
// covers the versions that hook either one.
function onKeydown(e: KeyboardEvent) {
  if (e.key === '/' && !open.value) {
    e.preventDefault()
    openAndFocus()
  } else if (e.key === 'Escape') {
    open.value = false
  }
}

function onKeypress(e: KeyboardEvent) {
  if (e.key === '/' && !open.value) {
    e.preventDefault()
  }
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown, { capture: true })
  window.addEventListener('keypress', onKeypress, { capture: true })
})
onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown, { capture: true })
  window.removeEventListener('keypress', onKeypress, { capture: true })
})

const results = computed(() => {
  const q = query.value.toLowerCase()
  if (!q) return []
  return files.value.filter((f) => f.toLowerCase().includes(q)).slice(0, 50)
})

function hrefFor(f: string) {
  return `/${props.repo}/blob/${props.gitRef}/${f}`
}
</script>

<template>
  <div class="search-island">
    <button class="search-toggle" @click="openAndFocus">search files (<kbd>/</kbd>)</button>
    <div class="search-panel" :class="{ hidden: !open }">
      <input
        ref="inputEl"
        v-model="query"
        type="text"
        placeholder="filter files..."
        @keydown.escape="open = false"
      />
      <ul class="search-results">
        <li v-for="f in results" :key="f">
          <NuxtLink :to="hrefFor(f)">{{ f }}</NuxtLink>
        </li>
      </ul>
    </div>
  </div>
</template>
