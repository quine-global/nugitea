<script setup lang="ts">
const props = defineProps<{
  /** Full owner path, e.g. `acme/platform`. */
  owner: string
  repo?: string
  gitRef?: string
  path?: string
}>()

const crumbs = computed(() => {
  const list = [{ label: 'repos', href: '/repos' }]
  let acc = ''
  for (const seg of props.owner.split('/')) {
    acc = acc ? `${acc}/${seg}` : seg
    list.push({ label: seg, href: `/${acc}` })
  }
  if (props.repo) {
    const repoHref = `/${props.owner}/${props.repo}`
    list.push({ label: props.repo, href: props.gitRef ? `${repoHref}/tree/${props.gitRef}` : repoHref })
    let pathAcc = ''
    if (props.path) {
      for (const seg of props.path.split('/')) {
        pathAcc = pathAcc ? `${pathAcc}/${seg}` : seg
        list.push({ label: seg, href: `${repoHref}/tree/${props.gitRef}/${pathAcc}` })
      }
    }
  }
  return list
})
</script>

<template>
  <nav class="breadcrumbs">
    <template v-for="(c, i) in crumbs" :key="c.href">
      <NuxtLink v-if="i < crumbs.length - 1" :to="c.href">{{ c.label }}</NuxtLink>
      <strong v-else>{{ c.label }}</strong>
      <span v-if="i < crumbs.length - 1"> / </span>
    </template>
  </nav>
</template>
