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
  inputEl.value?.focus()
  if (!files.value.length) {
    await loadFiles()
  }
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === '/' && !open.value) {
    e.preventDefault()
    openAndFocus()
  } else if (e.key === 'Escape') {
    open.value = false
  }
}

onMounted(() => window.addEventListener('keydown', onKeydown))
onUnmounted(() => window.removeEventListener('keydown', onKeydown))

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
