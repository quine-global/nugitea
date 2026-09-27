<script setup lang="ts">
interface ReposData {
  repositories: { nodes: { nameWithOwner: string }[] }
}

const { data } = await useAsyncData('repos', () =>
  graphqlRequest<ReposData>(`query { repositories { nodes { nameWithOwner } } }`)
)

const repos = computed(() => data.value?.repositories.nodes ?? [])
</script>

<template>
  <div class="container">
    <h2>Repositories</h2>
    <p v-if="!repos.length"><em>No repositories yet.</em></p>
    <ul v-else class="entries">
      <li v-for="r in repos" :key="r.nameWithOwner">
        <NuxtLink :to="`/${r.nameWithOwner}`">{{ r.nameWithOwner }}</NuxtLink>
      </li>
    </ul>
  </div>
</template>
