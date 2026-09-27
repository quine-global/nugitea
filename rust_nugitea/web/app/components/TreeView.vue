<script setup lang="ts">
const props = defineProps<{
  /** `nameWithOwner`, e.g. `acme/platform/api`. */
  repo: string
  gitRef: string
  path: string
}>()

const expression = computed(() => (props.path ? `${props.gitRef}:${props.path}` : props.gitRef))

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
  () => `tree-${props.repo}-${expression.value}`,
  () =>
    graphqlRequest<TreeData>(
      `query($owner: String!, $name: String!, $expr: String!) {
        repository(owner: $owner, name: $name) {
          object(expression: $expr) {
            __typename
            ... on Tree { entries { name type } }
          }
        }
      }`,
      { ...splitRepo(props.repo), expr: expression.value }
    ),
  { watch: [() => props.repo, expression] }
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
  const childPath = props.path ? `${props.path}/${entry.name}` : entry.name
  return `/${props.repo}/${entry.type === 'tree' ? 'tree' : 'blob'}/${props.gitRef}/${childPath}`
}
</script>

<template>
  <div class="container">
    <h2>{{ repo }}</h2>
    <Breadcrumbs :owner="splitRepo(repo).owner" :repo="splitRepo(repo).name" :git-ref="gitRef" :path="path" />
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
