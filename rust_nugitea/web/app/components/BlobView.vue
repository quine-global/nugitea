<script setup lang="ts">
const props = defineProps<{
  /** `nameWithOwner`, e.g. `acme/platform/api`. */
  repo: string
  gitRef: string
  path: string
}>()

const expression = computed(() => `${props.gitRef}:${props.path}`)

interface BlobData {
  repository: {
    object: { text: string | null; isBinary: boolean } | null
  } | null
}

const { data } = await useAsyncData(
  () => `blob-${props.repo}-${expression.value}`,
  () =>
    graphqlRequest<BlobData>(
      /* GraphQL */ `query($owner: String!, $name: String!, $expr: String!) {
        repository(owner: $owner, name: $name) {
          object(expression: $expr) {
            __typename
            ... on Blob { text isBinary }
          }
        }
      }`,
      { ...splitRepo(props.repo), expr: expression.value }
    ),
  { watch: [() => props.repo, expression] }
)

const blob = computed(() => data.value?.repository?.object)
</script>

<template>
  <div class="container">
    <h2>{{ repo }}</h2>
    <Breadcrumbs :owner="splitRepo(repo).owner" :repo="splitRepo(repo).name" :git-ref="gitRef" :path="path" />
    <SearchBox :repo="repo" :git-ref="gitRef" />
    <p v-if="!blob">Not found.</p>
    <p v-else-if="blob.isBinary"><em>binary file not shown</em></p>
    <pre v-else>{{ blob.text }}</pre>
  </div>
</template>
