// Deliberately no GraphQL client library (Apollo, urql, ...) — this UI is
// read-only and doesn't need query caching/normalization, so a plain
// $fetch POST is the whole client. Matches the Rust side's habit of
// reaching for the minimal tool that does the job.

/** The GraphQL endpoint: the server-only value during SSR (reaches the
 * app tier over the Docker-internal network), the public value once
 * running in the browser (must be reachable from the user's machine). */
export function useGraphqlUrl(): string {
  const config = useRuntimeConfig()
  return import.meta.server ? config.graphqlUrl : config.public.graphqlUrl
}

interface GraphqlResponse<T> {
  data?: T
  errors?: Array<{ message: string }>
}

export async function graphqlRequest<T>(query: string, variables?: Record<string, unknown>): Promise<T> {
  const url = useGraphqlUrl()
  const res = await $fetch<GraphqlResponse<T>>(url, {
    method: 'POST',
    body: { query, variables }
  })
  if (res.errors?.length) {
    throw new Error(res.errors.map((e) => e.message).join('; '))
  }
  if (!res.data) {
    throw new Error('GraphQL response had no data')
  }
  return res.data
}
