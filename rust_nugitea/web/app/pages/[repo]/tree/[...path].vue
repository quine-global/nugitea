<script setup lang="ts">
const route = useRoute()
const repo = computed(() => route.params.repo as string)
const segments = computed(() => route.params.path as string[])
const gitRef = computed(() => segments.value[0])
const path = computed(() => segments.value.slice(1).join('/'))
const expression = computed(() => (path.value ? `${gitRef.value}:${path.value}` : gitRef.value))

interface TreeEntry {
  name: string
  type: string
}
interface TreeData {
  repository: {
    object: { entries: TreeEntry[] } | null
  } | null
}

const { data } = await useAsyncData(
  () => `tree-${repo.value}-${expression.value}`,
  () =>
    graphqlRequest<TreeData>(
      `query($name: String!, $expr: String!) {
        repository(name: $name) {
          object(expression: $expr) {
            __typename
            ... on Tree { entries { name type } }
          }
        }
      }`,
      { name: repo.value, expr: expression.value }
    ),
  { watch: [expression] }
)

const entries = computed(() => {
  const list = data.value?.repository?.object?.entries ?? []
  return [...list].sort((a, b) => {
    const kindOrder = (t: string) => (t === 'blob' ? 1 : 0)
    const byKind = kindOrder(a.type) - kindOrder(b.type)
    return byKind !== 0 ? byKind : a.name.localeCompare(b.name)
  })
})

function childHref(entry: TreeEntry) {
  const childPath = path.value ? `${path.value}/${entry.name}` : entry.name
  return `/${repo.value}/${entry.type === 'tree' ? 'tree' : 'blob'}/${gitRef.value}/${childPath}`
}
</script>

<template>
  <div class="container">
    <h2>{{ repo }}</h2>
    <Breadcrumbs :repo="repo" :git-ref="gitRef" :path="path" />
    <SearchBox :repo="repo" :git-ref="gitRef" />
    <p v-if="!data?.repository?.object">Not found.</p>
    <p v-else-if="!entries.length"><em>(empty directory)</em></p>
    <ul v-else class="entries">
      <li v-for="e in entries" :key="e.name">
        {{ e.type === 'tree' ? '📁' : '📄' }}
        <NuxtLink :to="childHref(e)">{{ e.name }}</NuxtLink>
      </li>
    </ul>
  </div>
</template>
