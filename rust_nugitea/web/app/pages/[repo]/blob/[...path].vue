<script setup lang="ts">
const route = useRoute()
const repo = computed(() => route.params.repo as string)
const segments = computed(() => route.params.path as string[])
const gitRef = computed(() => segments.value[0])
const path = computed(() => segments.value.slice(1).join('/'))
const expression = computed(() => `${gitRef.value}:${path.value}`)

interface BlobData {
  repository: {
    object: { text: string | null; isBinary: boolean } | null
  } | null
}

const { data } = await useAsyncData(
  () => `blob-${repo.value}-${expression.value}`,
  () =>
    graphqlRequest<BlobData>(
      `query($name: String!, $expr: String!) {
        repository(name: $name) {
          object(expression: $expr) {
            __typename
            ... on Blob { text isBinary }
          }
        }
      }`,
      { name: repo.value, expr: expression.value }
    ),
  { watch: [expression] }
)

const blob = computed(() => data.value?.repository?.object)
</script>

<template>
  <div class="container">
    <h2>{{ repo }}</h2>
    <Breadcrumbs :repo="repo" :git-ref="gitRef" :path="path" />
    <SearchBox :repo="repo" :git-ref="gitRef" />
    <p v-if="!blob">Not found.</p>
    <p v-else-if="blob.isBinary"><em>binary file not shown</em></p>
    <pre v-else>{{ blob.text }}</pre>
  </div>
</template>
