# web

nugitea's browsing UI: a Nuxt app that speaks only GraphQL to the app
tier. See `../README.md` for how it fits with the other two processes and
how to run it.

```sh
npm install
npm run dev            # needs NUXT_GRAPHQL_URL / NUXT_PUBLIC_GRAPHQL_URL pointing at the app tier
npm run typecheck
npm run format         # or format:check
```

`schema.graphql` is generated from the Rust side — don't edit it by hand
(see "Development tooling" in `../README.md`).
