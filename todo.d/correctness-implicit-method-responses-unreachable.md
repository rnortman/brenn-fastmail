A reader for the response to an implicit method call. `Email/copy` with
`onSuccessDestroyOriginal` (RFC 8620 section 5.4) and `EmailSubmission/set`
with `onSuccessUpdateEmail` or `onSuccessDestroyEmail` (RFC 8621 section 7.5)
make the server call `Email/set` itself; its response follows the primary one
under the same call id. `Response::result` reads only the first response for an
id, so a `notUpdated` or `notDestroyed` in the implicit `Email/set`, or an
`error` in its place, is unread today. The first cycle that sends such a call
adds the reader (matching the id and the implicit method's name, an `error`
after the primary response being the implicit call's failure) and decides what
a failed implicit call means to the tool's caller. The
`TODO(correctness-implicit-method-responses-unreachable)` comment sits above
`Response::result` in `jmap/src/response.rs`.
