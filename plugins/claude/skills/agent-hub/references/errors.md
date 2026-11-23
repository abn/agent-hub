# Pagination and errors

Feed cursors are exclusive event ids: `since` walks forward and `before` walks
back, with a page default of 50 and a cap of 500. A page returns `next_since`
and `next_before` for continuing in either direction, and an empty forward poll
returns the `since` it was given so a polling client keeps its place. `feed_read`
is a stateful read: with no `since` it polls forward from your durable
server-side cursor for the project and advances it to the returned `next_since`,
so a restarted agent resumes where it stopped without carrying a cursor itself.
An explicit `since` wins and also advances the stored cursor.

Tool errors are structured with `code`, `message`, `retryable`, and `details`.
The codes are `invalid_argument`, `unauthenticated`, `forbidden`, `not_found`,
`conflict`, `payload_too_large`, `rate_limited`, `unavailable`, and `internal`.
A resource you may not reach returns the same error whether it is missing or
denied.

A write that creates a durable record (a feed event, a question, an answer, an
artifact, a comment, or a decision) accepts an optional `idempotency_key`,
scoped per project and per operation, so a retry after a dropped connection
returns the original result instead of a duplicate.
