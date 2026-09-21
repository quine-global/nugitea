<script setup lang="ts">
const props = defineProps<{
  repo: string
  gitRef: string
  path: string
}>()

const crumbs = computed(() => {
  const list = [{ label: props.repo, href: `/${props.repo}/tree/${props.gitRef}` }]
  let acc = ''
  if (props.path) {
    for (const seg of props.path.split('/')) {
      acc = acc ? `${acc}/${seg}` : seg
      list.push({ label: seg, href: `/${props.repo}/tree/${props.gitRef}/${acc}` })
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
