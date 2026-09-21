<script setup lang="ts">
const route = useRoute()
const repo = route.params.repo as string

interface RepoRootData {
  repository: { defaultBranchRef: { name: string } | null } | null
}

const { data } = await useAsyncData(`repo-root-${repo}`, () =>
  graphqlRequest<RepoRootData>(
    `query($name: String!) { repository(name: $name) { defaultBranchRef { name } } }`,
    { name: repo }
  )
)

const defaultBranch = data.value?.repository?.defaultBranchRef?.name
if (defaultBranch) {
  await navigateTo(`/${repo}/tree/${defaultBranch}`)
}
</script>

<template>
  <div class="container">
    <h2>{{ repo }}</h2>
    <p v-if="!data?.repository">Repository not found.</p>
    <p v-else-if="!data.repository.defaultBranchRef">This repository has no branches yet.</p>
  </div>
</template>
