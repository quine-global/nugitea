<script setup lang="ts">
// Every owner and repo URL lands here, because nested orgs make the
// owner part of the path any depth (`/acme/platform/api/tree/main/src`),
// so there's no fixed route shape to split it on. Instead this asks the
// API what the path names — `resource(url:)` walks owner segments until
// one is a repo — and whatever follows the resource's own path is the
// GitHub-style `tree/<ref>/<path>` or `blob/<ref>/<path>` suffix.

const route = useRoute()
const segments = computed(() => ([] as string[]).concat(route.params.slug ?? []))
const url = computed(() => `/${segments.value.join('/')}`)

type Resource =
  | { __typename: 'Repository'; resourcePath: string; nameWithOwner: string; defaultBranchRef: { name: string } | null }
  | { __typename: 'User' | 'Organization'; resourcePath: string }

const { data } = await useAsyncData(
  () => `resource-${url.value}`,
  () =>
    graphqlRequest<{ resource: Resource | null }>(
      /* GraphQL */ `query($url: String!) {
        resource(url: $url) {
          __typename
          resourcePath
          ... on Repository { nameWithOwner defaultBranchRef { name } }
        }
      }`,
      { url: url.value }
    ),
  { watch: [url] }
)

const resource = computed(() => data.value?.resource ?? null)
const repo = computed(() => (resource.value?.__typename === 'Repository' ? resource.value : null))

/** The URL segments after the resource's own path. */
const rest = computed(() => {
  if (!resource.value) return []
  const depth = resource.value.resourcePath.split('/').filter(Boolean).length
  return segments.value.slice(depth)
})

const view = computed(() => {
  const r = resource.value
  if (!r) return null
  if (r.__typename !== 'Repository') {
    return rest.value.length ? null : { kind: 'owner' as const, login: r.resourcePath.slice(1) }
  }
  const [kind, gitRef, ...path] = rest.value
  if (!kind) return { kind: 'repo-root' as const }
  if (kind === 'tree' && gitRef) {
    return { kind: 'tree' as const, repo: r.nameWithOwner, gitRef, path: path.join('/') }
  }
  if (kind === 'blob' && gitRef && path.length) {
    return { kind: 'blob' as const, repo: r.nameWithOwner, gitRef, path: path.join('/') }
  }
  return null
})

// The bare repo URL redirects to its default branch's tree, and `/`
// (which this catch-all also matches) to the repo listing.
async function redirect() {
  if (!segments.value.length) {
    await navigateTo('/repos')
  } else if (view.value?.kind === 'repo-root' && repo.value?.defaultBranchRef) {
    await navigateTo(`${repo.value.resourcePath}/tree/${repo.value.defaultBranchRef.name}`)
  }
}
await redirect()
watch(view, redirect)
</script>

<template>
  <OwnerView v-if="view?.kind === 'owner'" :login="view.login" />
  <TreeView v-else-if="view?.kind === 'tree'" :repo="view.repo" :git-ref="view.gitRef" :path="view.path" />
  <BlobView v-else-if="view?.kind === 'blob'" :repo="view.repo" :git-ref="view.gitRef" :path="view.path" />
  <div v-else class="container">
    <template v-if="repo && view?.kind === 'repo-root'">
      <h2>{{ repo.nameWithOwner }}</h2>
      <p>This repository has no branches yet.</p>
    </template>
    <p v-else>Not found.</p>
  </div>
</template>
