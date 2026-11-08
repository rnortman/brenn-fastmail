Attachment download and blob upload. Fastmail serves blobs from a download
host other than the JMAP API host, so a provider that fetches an attachment
needs that host as a second `api_hosts` entry on the key provider, and a
binary body on the provider kit's call type, which the kit does not carry
today; both are later work. Until then `FastmailRead` lists each attachment's
metadata and fetches none. The `TODO(blob-transfer)` comment sits at the
attachment emitter in `fastmail-mail-read/src/logic.rs`.
