<script setup lang="ts">
const props = defineProps<{
  /** A user's login or an org's full path (`acme/platform`). */
  login: string
}>()

interface OwnerData {
  repositoryOwner: {
    __typename: 'User' | 'Organization'
    login: string
    repositories: { nodes: { name: string; nameWithOwner: string }[] }
    subOrganizations?: { nodes: { login: string }[] }
  } | null
}

const { data } = await useAsyncData(
  () => `owner-${props.login}`,
  () =>
    graphqlRequest<OwnerData>(
      /* GraphQL */ `query($login: String!) {
        repositoryOwner(login: $login) {
          __typename
          login
          repositories { nodes { name nameWithOwner } }
          ... on Organization { subOrganizations { nodes { login } } }
        }
      }`,
      { login: props.login }
    ),
  { watch: [() => props.login] }
)

const owner = computed(() => data.value?.repositoryOwner)
const subOrgs = computed(() => owner.value?.subOrganizations?.nodes ?? [])
const repos = computed(() => owner.value?.repositories.nodes ?? [])
</script>

<template>
  <div class="container">
    <h2>{{ owner?.login ?? login }}</h2>
    <Breadcrumbs :owner="owner?.login ?? login" />
    <p v-if="!owner">Not found.</p>
    <p v-else-if="!subOrgs.length && !repos.length"><em>Nothing here yet.</em></p>
    <ul v-else class="entries">
      <li v-for="o in subOrgs" :key="o.login">
        🏢 <NuxtLink :to="`/${o.login}`">{{ o.login.slice(o.login.lastIndexOf('/') + 1) }}</NuxtLink>
      </li>
      <li v-for="r in repos" :key="r.nameWithOwner">
        📦 <NuxtLink :to="`/${r.nameWithOwner}`">{{ r.name }}</NuxtLink>
      </li>
    </ul>
  </div>
</template>
