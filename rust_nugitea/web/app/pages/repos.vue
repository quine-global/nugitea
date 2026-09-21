<script setup lang="ts">
interface ReposData {
  repositories: { name: string }[]
}

const { data } = await useAsyncData('repos', () =>
  graphqlRequest<ReposData>(`query { repositories { name } }`)
)

const repos = computed(() => data.value?.repositories ?? [])
</script>

<template>
  <div class="container">
    <h2>Repositories</h2>
    <p v-if="!repos.length"><em>No repositories yet.</em></p>
    <ul v-else class="entries">
      <li v-for="r in repos" :key="r.name">
        <NuxtLink :to="`/${r.name}`">{{ r.name }}</NuxtLink>
      </li>
    </ul>
  </div>
</template>
