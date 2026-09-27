/** Splits a `nameWithOwner` into the `owner`/`name` pair that
 * `repository(owner:, name:)` takes. The owner is everything before the
 * last `/`, since nested orgs make it a path of its own
 * (`acme/platform/api` -> `acme/platform`, `api`). */
export function splitRepo(nameWithOwner: string): { owner: string; name: string } {
  const i = nameWithOwner.lastIndexOf('/')
  return { owner: nameWithOwner.slice(0, i), name: nameWithOwner.slice(i + 1) }
}
