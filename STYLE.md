This document outlines the coding style.

1. Code should be minimal.
2. Abstraction should be simple, small and easy to read.
3. Abstraction does exactly one thing.
4. don't let a = b.c; fncall(a); do it inline
5. minimize the amount of state combinations - do we need this optional? do we need mutex?
6. abstractions shouldn't mix levels. One abstraction shouldn't have leaked logic from abstraction underneath.
7. Don't swallow errors. No .is_err() -> continue. and no .unwrap_or_default. in most cases it's either the optional can be removed or use "?" and forward the error to the better top level error handler.
8. Don't create new types if it's not needed. Abstraction are welcome, but if the abstraction is just couple lines of code it's better to inline it in the caller.
9. USE abstractions. Software should be layered with one abstraction on multiple others. That's how we get composable and maintanble architecture.
10. Don't introduce new state if thing can be computed inline.
11. Don't write too many comments. And comments you write should be useful for the external user. 
12. All abstractions have clear responsibilities and documentation (and it's contracts). And any abstraction can be learned easily by looking at it and studying the documentation of it's dependencies. 
13. Avoid smart pointers. Arc, Mutex and others should be a last resort. It's easy to get a softlock and its hard to track where they go. Rust has decent memory model, use it.
14. Use other peoples work. Utilize crates, macros, external helpers. 
