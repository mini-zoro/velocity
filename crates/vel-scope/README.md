# vel-scope

Global, thread-safe interning of dotted Sublime scope names. `Scope` and `Atom` IDs are
`Copy`; scope sequences have no eight-atom limit. Serialized scopes contain names,
so caches do not depend on a process's interning order.

`ScopeStack::apply` implements push/pop, nested `clear_scopes` and restore. A clear
saves even an empty scope suffix, so nested restores remain balanced.

`Selector::parse` supports descendant paths, immediate children (`>` and repeated
`>`), negation, intersection, grouping, and both alternation operators with the
documented precedence. Matching an already parsed selector allocates nothing.
Numeric specificity uses deeper scope positions and longer atom prefixes; the
numeric formula is **unverified ST behavior**, marked in the source. Repeated or
trailing dots are normalized for selectors present in the public package tests.

Written for Velocity from the public
[selector documentation](https://www.sublimetext.com/docs/selectors.html) and
[scope naming documentation](https://www.sublimetext.com/docs/scope_naming.html).
No private editor assets or binaries are used.
