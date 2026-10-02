# Security

RageLab parses untrusted binary files and must treat malformed input as hostile.

## Reporting

Please report security-sensitive issues privately to the repository maintainers rather than opening a public issue with exploit details.

## Scope

Security-sensitive areas include:

- unchecked offsets, pointers, lengths, or integer conversions;
- parser panics reachable from malformed input;
- path traversal or unsafe filesystem writes;
- writer behavior that can overwrite source assets unexpectedly;
- archive or workspace extraction outside the intended root;
- machine-readable contracts that expose unintended local data.

## Engineering policy

RageLab forbids unsafe Rust at workspace level. Parsing and writing code should use explicit bounds checks and typed errors. Unsupported binary layouts must fail closed.
