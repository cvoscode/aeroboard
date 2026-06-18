# Rust Code Smells and Anti-Patterns

**Executive Summary:** Rust’s strict ownership and type system prevents many bugs, but developers coming from other languages often run into anti-patterns—*“seductive shortcuts”* that degrade performance or safety【22†L66-L74】. This report catalogues 18+ common Rust code smells (ownership/borrowing, lifetimes, error handling, etc.), each with a minimal wrong example and an idiomatic fix. We focus on idiomatic Rust (references from the official Rust book/Rustonomicon and well-known community sources). For example, using `clone` to satisfy the borrow checker or calling `unwrap` in production are classic anti-patterns【18†L57-L65】【30†L939-L944】. We also illustrate ownership/borrowing flows and async-blocking pitfalls with small diagrams.

```mermaid
flowchart LR
    Owner["Owner (data)"] -->|&T (shared borrow)| Shared["&T (immutable borrow)"]
    Owner -->|&mut T (unique borrow)| Unique["&mut T (mutable borrow)"]
    Shared --> Use["Use (read-only)"]
    Unique --> Mod["Modify"]
```
*Figure: Rust allows either many immutable borrows or one mutable borrow at a time【2†L79-L87】, ensuring safe aliasing. Use shared (`&T`) for reading and unique (`&mut T`) for writing (no simultaneous mutable+immutable).*

```mermaid
flowchart TB
    AsyncTask["async task"] -->|`.await`| AsyncOp["Future executed"]
    AsyncTask -->|`std::thread::sleep()`| Blocker["Blocks executor thread"]
    Blocker --> ExecutorBack["Async executor starves"]
    AsyncTask -->|`spawn_blocking()`| NewThread["CPU task in thread pool"]
    NewThread -->|`heavy work`| Complete["Completes without blocking async executor"]
```
*Figure: Blocking operations (e.g. `std::thread::sleep`) inside async code block the executor【35†L75-L78】. Instead, use async alternatives (like `tokio::time::sleep(...).await`) or offload heavy work via `spawn_blocking`【35†L105-L109】.*

| **Anti-pattern / Smell**                           | **Example (anti-pattern)**                              | **Right way (idiomatic)**                                                                                                                                                      |
|-----------------------------------------------|-------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------|
| **Unnecessary `clone()`**<br/>(Clone-to-borrow) | ```rust  
let mut x = 5;
let y = &mut (x.clone());
*y += 1;
``` | ```rust
let mut x = 5;
let y = &mut x;
*y += 1;
``` Avoid cloning to satisfy the borrow checker. Cloning makes a separate copy (updates to one don’t affect the other)【18†L57-L65】. Instead, borrow or move the original data directly. |
| **Unnecessary Allocation (`String` vs `&str`)** | ```rust  
fn greet(name: String) -> String {
    format!("Hello, {}", name)
}
greet("Alice".to_string());
``` | ```rust
fn greet(name: &str) -> String {
    format!("Hello, {}", name)
}
let s = "Alice";
greet(s);
``` Take `&str` parameters when you don’t need ownership. Requiring an owned `String` forces callers to allocate. Using `&str` avoids needless heap allocation【24†L438-L441】. Return `String` only when transferring ownership. |
| **Unchecked `unwrap()` / `panic!`**          | ```rust  
let data = std::fs::read_to_string("config.json").unwrap(); // panics if missing
let user_age = users.get("alice").unwrap().age;            // panics if not found
``` | ```rust
fn load_config(path: &str) -> Result<Config, std::io::Error> {
    let contents = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&contents)?;
    Ok(config)
}
match users.get("alice") {
    Some(user) => println!("Age: {}", user.age),
    None => println!("User not found"),
}
``` Avoid using `unwrap()` or `expect()` on runtime data; these cause panics on invalid input. Instead return or propagate a `Result`/`Option`. Panics yield cryptic errors and crash programs【30†L939-L944】. Reserve `unwrap` only when you are certain (e.g. parsing hardcoded literals or in tests). |
| **Mixed Immutable/Mutable Borrow**            | ```rust  
let mut vec = vec![1, 2, 3];
let first = &vec[0];   // immutable borrow
vec.push(4);          // mutable borrow -> compile error
println!("{}", first);
``` | ```rust
let mut vec = vec![1, 2, 3];
let first_val = vec[0];       // copy the value
vec.push(4);                  // now safe to mutate
println!("{}", first_val);
``` Do not hold an immutable borrow while mutating the same data; Rust forbids this【2†L79-L87】. Restructure the code (e.g. limit the lifetime of the immutable reference or take a copy of the value) so that the mutable borrow and immutable borrow don’t overlap. |
| **Dangling Lifetime / Borrow from Local**    | ```rust
fn longest<'a>(x: &'a str, y: &'a str) -> &'a str { 
    if x.len() > y.len() { x } else { y }
}
fn main() {
    let r;
    {
        let s1 = String::from("long string");
        let s2 = "short";
        r = longest(&s1, s2);  // error: s1 does not live long enough
    }
    println!("Longest: {}", r);
}
``` | ```rust
fn longest<'a>(x: &'a str, y: &'a str) -> &'a str { 
    if x.len() > y.len() { x } else { y }
}
fn main() {
    let s1 = String::from("long string");
    let s2 = "short";
    let r = longest(&s1, s2);  // both s1 and s2 live long enough here
    println!("Longest: {}", r);
}
``` Ensure that references do not outlive their data. In the bad example, `s1` is dropped before `r` is used, causing a compile-time error. The fix is to adjust scopes so the referenced data lives long enough (or avoid returning references tied to short-lived values)【2†L115-L123】. |
| **Manual Loops vs Iterator Combinators**     | ```rust  
fn evens_double(nums: &[i32]) -> Vec<i32> {
    let mut res = Vec::new();
    for &x in nums {
        if x % 2 == 0 {
            res.push(x * 2);
        }
    }
    res
}
``` | ```rust
fn evens_double(nums: &[i32]) -> Vec<i32> {
    nums.iter()
        .filter(|&&x| x % 2 == 0)
        .map(|&x| x * 2)
        .collect()
}
``` Prefer iterator methods (`filter`, `map`, etc.) over manual loops. Manual loops are more verbose and error-prone, whereas iterator chains are declarative and can be optimized by the compiler【24†L298-L305】. They also compose more easily and can be parallelized (e.g. with Rayon). |
| **Collecting Iterators Too Early**           | ```rust  
let evens: Vec<_> = numbers.iter().filter(|&&x| x%2==0).copied().collect();
let sum: i32 = evens.iter().map(|&x| x*2).sum();
``` | ```rust
let sum: i32 = numbers.iter()
    .filter(|&&x| x % 2 == 0)
    .map(|&x| x * 2)
    .sum();
``` Avoid inserting `.collect()` in the middle of an iterator chain if you only need a final value (like a sum). Intermediate `collect` causes extra heap allocations and breaks iterator fusion【26†L508-L516】. Instead chain iterators directly (here `.sum()` combines filtering and mapping in one pass). |
| **`Box<dyn Trait>` vs Generics**             | ```rust  
trait Processor { fn proc(&self, s: &str) -> String; }
fn run(ps: Vec<Box<dyn Processor>>, input: &str) -> String {
    let mut out = input.to_string();
    for p in ps {
        out = p.proc(&out);
    }
    out
}
``` | ```rust
trait Processor { fn proc(&self, s: &str) -> String; }
fn run<P: Processor, Q: Processor, R: Processor>(p1: P, p2: Q, p3: R, input: &str) -> String {
    let out1 = p1.proc(input);
    let out2 = p2.proc(&out1);
    p3.proc(&out2)  // calls can be inlined, no heap
}
``` Use static dispatch (generics/`impl Trait`) when possible instead of `Box<dyn Trait>`. Each trait object allocates on the heap and incurs a virtual-call cost【32†L778-L782】. If the concrete types are known at compile time, generics produce faster, inlined code. Reserve `dyn Trait` for when you truly need runtime polymorphism. |
| **`unsafe` for Borrow-Cheater**               | ```rust
struct Cache { data: Vec<String> }
impl Cache {
    fn get2(&mut self, i: usize, j: usize) -> (&mut String, &mut String) {
        unsafe {
            let ptr = self.data.as_mut_ptr();
            (&mut *ptr.add(i), &mut *ptr.add(j))
        }
    }
}
``` | ```rust
struct Cache { data: Vec<String> }
impl Cache {
    fn get2(&mut self, i: usize, j: usize) -> Option<(&mut String,&mut String)> {
        if i == j || i >= self.data.len() || j >= self.data.len() { return None; }
        if i < j {
            let (left,right) = self.data.split_at_mut(j);
            Some((&mut left[i], &mut right[0]))
        } else {
            let (left,right) = self.data.split_at_mut(i);
            Some((&mut right[0], &mut left[j]))
        }
    }
}
``` Don’t use `unsafe` to bypass the borrow checker “just because you can”. In the bad example, if `i == j` this creates two mutable aliases, which is undefined behavior【30†L855-L862】. Instead use safe methods like `split_at_mut` or checked indexing to ensure you never create overlapping mutable references. |
| **RefCell/Mutex by Default**                  | ```rust
struct App {
    state: RefCell<State>,
    count: RefCell<u32>,
}
impl App {
    fn incr(&self) {
        let mut s = self.state.borrow_mut();
        *s += 1;
        *self.count.borrow_mut() += 1;
    }
}
``` | ```rust
struct App { state: State, count: u32 }
impl App {
    fn incr(&mut self) {
        self.state += 1;
        self.count += 1;
    }
}
``` Don’t use interior mutability (`RefCell`/`Mutex`) as a first resort. The example above hides mutation behind `&self` and adds runtime borrow checks (and potential panics)【28†L1018-L1026】. Instead design APIs that take `&mut self` when mutating, yielding compile-time borrow checking. Use `RefCell`/`Mutex` only when genuinely needed (e.g. shared data structures, caching, or multi-threaded access). |
| **Ignoring Send/Sync (Data Races)**           | ```rust
use std::rc::Rc;
use std::thread;
let data = Rc::new(RefCell::new(vec![1,2,3]));
thread::spawn(move || {
    // attempt to share Rc<RefCell<...>> across threads
    data.borrow_mut().push(4);  // WRONG: not Send/Sync!
});
``` | ```rust
use std::sync::{Arc, Mutex};
use std::thread;
let data = Arc::new(Mutex::new(vec![1,2,3]));
let d = Arc::clone(&data);
thread::spawn(move || {
    let mut v = d.lock().unwrap();
    v.push(4);    // Safe: Arc<Mutex<_>> is Send+Sync
});
``` Never share non-`Send`/`Sync` types (like `Rc`, `RefCell`) across threads. The bad code would not compile, or if forced via unsafe it leads to data races and UB【28†L1108-L1116】. Use `Arc<Mutex<T>>` (or channels) for shared mutable data. In short: obey thread-safety. |
| **Forgetting `.await` in async**              | ```rust
async fn foo() -> i32 { 42 }
async fn main() {
    foo();                     // Forgot `.await`! Future is not executed
    let x = foo().await;       // Correct
    println!("{}", x);
}
``` | The first call to `foo()` does nothing because the future is never awaited. Always `.await` async calls to execute them【35†L117-L125】. If you need concurrency, use `tokio::spawn(async { foo().await; })` or `.await` directly. |
| **Blocking in async (sync calls)**           | ```rust
async fn handle() {
    std::thread::sleep(std::time::Duration::from_secs(1)); // Blocks executor
    println!("Done");
}
``` | ```rust
async fn handle() {
    tokio::time::sleep(std::time::Duration::from_secs(1)).await; // Non-blocking
    println!("Done");
}
``` Don’t call blocking functions (like `std::thread::sleep`, blocking file I/O, or heavy CPU loops) inside async code; they stall the async executor【35†L75-L78】. Use async-compatible versions (`tokio::time::sleep`, async file APIs) or offload work with `spawn_blocking`【35†L105-L109】. |
| **Spawning too many tasks**                  | ```rust
async fn work() { /* cheap work */ }
async fn main() {
    let mut handles = Vec::new();
    for i in 0..10000 {
        let h = tokio::spawn(work());  // spawning for trivial tasks
        handles.push(h);
    }
    for h in handles { h.await.unwrap(); }
}
``` | ```rust
async fn work() { /* cheap work */ }
async fn main() {
    for i in 0..10000 {
        work().await;  // just await directly for lightweight tasks
    }
}
``` Spawning a separate task for a trivial computation adds huge scheduling overhead【35†L175-L184】. Only use `spawn` for genuinely independent or CPU-intensive tasks. For small, cheap work, simply `await` the async function directly. |
| **Overusing `Rc`/`Arc`**                     | ```rust
use std::rc::Rc;
struct Config { /* ... */ }
struct App { cfg: Rc<Config> }
fn main() {
    let cfg = Rc::new(Config{});
    let app = App { cfg: Rc::clone(&cfg) }; // unnecessary Rc if only single owner
}
``` | ```rust
struct Config { /* ... */ }
struct App { cfg: Config }
fn main() {
    let cfg = Config {};
    let app = App { cfg };  // move ownership instead
}
``` Don’t wrap things in `Rc`/`Arc` unless you need shared ownership. Using `Rc` when only one owner exists adds heap/refcount overhead【22†L178-L185】. Prefer plain values (or use references) and add `Rc`/`Arc` only when multiple parts truly share data. |
| **Inefficient String Concatenation**        | ```rust
let parts = vec!["a", "b", "c"];
let mut s = String::new();
for p in parts {
    s = s + p;    // creates a new String each iteration
}
``` | ```rust
let parts = vec!["a", "b", "c"];
let s = parts.join("");  // join efficiently concatenates
``` Using `s = s + p` in a loop reallocates each time. It’s better to use `parts.join("")` or accumulate via `write!` or `push_str`. The `join` method concatenates in one go and is far more efficient【4†L213-L220】. |
| **`Vec` for Fixed Small Data**              | ```rust
fn rgb(pixel: u32) -> Vec<u8> {
    vec![
        ((pixel>>16)&0xFF) as u8,
        ((pixel>>8)&0xFF) as u8,
        (pixel&0xFF) as u8,
    ]
}
``` | ```rust
fn rgb(pixel: u32) -> [u8;3] {
    [
        ((pixel>>16)&0xFF) as u8,
        ((pixel>>8)&0xFF) as u8,
        (pixel&0xFF) as u8,
    ]
}
``` For fixed-size collections (like 3-byte RGB channels), use an array (`[u8;3]`) instead of `Vec<u8>`. A `Vec` allocates on the heap and has pointer overhead【26†L563-L571】. Fixed-size arrays are stack-allocated, faster, and more cache-friendly. |
| **`HashMap` for Few Keys**                 | ```rust
fn code_for_status(s: &str) -> u16 {
    let mut m = std::collections::HashMap::new();
    m.insert("ok", 200);
    m.insert("not_found", 404);
    m.insert("error", 500);
    *m.get(s).unwrap_or(&500)
}
``` | ```rust
fn code_for_status(s: &str) -> u16 {
    match s {
        "ok"         => 200,
        "not_found"  => 404,
        "error"      => 500,
        _ => 500,
    }
}
``` If you have only a few known keys, a `match` (or array lookup) is faster than rebuilding a `HashMap` each time【26†L629-L638】. For <10 items, hashing overhead dominates. Use `match` or a static array for small sets. |
| **Pattern Matching Instead of Manual `is_some()`** | ```rust
let opt = Some(5);
if opt.is_some() {
    println!("{}", opt.unwrap());
}
``` | ```rust
if let Some(v) = opt {
    println!("{}", v);
}
``` Prefer `if let` or `match` on `Option`/`Result` directly, rather than checking and then calling `unwrap`. The idiomatic way is: `if let Some(v) = opt { … }`. This avoids redundant unwraps and makes code clearer. |

**Summary of Fixes:**  In general, idiomatic Rust favors *borrowing* over cloning, explicit error returns over panics, iterator adapters over manual loops, and compile-time checks over runtime cost. We recommend using linters like Clippy and following official guidelines (Rust Book, Rustonomicon) to catch many of these issues【18†L57-L65】【30†L939-L944】.

**Sources:** Rust community wisdom and documentation (Rust Book, Rustonomicon) highlight these anti-patterns【22†L66-L74】【30†L914-L922】. The examples and advice above are drawn from those authoritative sources and well-known blog posts【2†L79-L87】【35†L117-L125】, ensuring both correctness and Rust idioms. Each table entry cites specific guidance from these sources.