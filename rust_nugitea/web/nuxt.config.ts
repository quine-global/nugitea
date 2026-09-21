// https://nuxt.com/docs/api/configuration/nuxt-config
export default defineNuxtConfig({
  compatibilityDate: '2025-07-15',
  devtools: { enabled: true },
  css: ['~/assets/main.css'],
  runtimeConfig: {
    // Server-only: used for SSR-time fetches, which happen container-to-
    // container in Docker Compose and so need the internal service name.
    graphqlUrl: 'http://127.0.0.1:3080/graphql',
    public: {
      // Client-visible: used for post-hydration fetches from the
      // browser, which need a URL the browser can actually reach.
      graphqlUrl: 'http://localhost:3080/graphql'
    }
  }
})
