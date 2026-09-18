# wasmtest stage plan

Tick a stage when `wasmtest --runtime my_runtime --stage N` is green. `wasmtest --list`
reads these boxes. Stages marked **[ext]** go beyond the core track (`--skip-ext` hides
them). All 45 stages are implemented — 347 tests — so no entry says **(planned)**;
each one names its source file and its test count. See README.md, "Adding a stage".

Run one stage: `wasmtest --runtime my_runtime --stage 5` — everything so far: `--until 12` —
the lot: `--all`. Prove the suite itself: `wasmtest --runtime wasmtime --validate --all`.

Every stage also carries worked **examples** — 92 of them — a module this suite
encodes, its bytes annotated section by section, and the output a correct runtime prints.
They live in the stage's own file and reach the site through `catalog.json`; nothing has to
be captured from a running runtime, because a module is input rather than a conversation.
See README.md, "Examples".

## A. Binary format & decoding

- [ ] **Stage 01** — The magic number and the version (`src/stages/s01_magic_and_version.rs`, 8 tests)
  - A module begins with the four bytes 00 61 73 6d — a NUL followed by 'asm' — and then a four-byte version
  - The version is 01 00 00 00: little-endian 1, not the big-endian 00 00 00 01 a network protocol would write
  - Check both before you decode anything else, and refuse the module without running a single instruction
  - A file shorter than those eight bytes is not a truncated module, it is not a module at all
- [ ] **Stage 02** — LEB128 integers and their edges (`src/stages/s02_leb128.rs`, 8 tests)
  - Read a uLEB128 seven bits at a time, lowest group first, and stop at the first byte whose top bit is clear — the count of bytes is not fixed and nothing tells you it in advance
  - A u32 field may legally take up to five bytes and a u64 up to ten, so accept padding; refuse a sixth (or eleventh) byte and refuse a value whose bits run past the field's width
  - Signed operands like the one in i32.const are sLEB128: sign-extend the last group from its bit 6, which is why i32.const -1 is the single byte 7f
  - A LEB128 that reaches the end of the file with its continuation bit still set is a malformed module, not a zero
- [ ] **Stage 03** — Section order, duplicates and unknown ids (`src/stages/s03_section_framing.rs`, 8 tests)
  - Read sections in a loop: one id byte, a uLEB128 size, then exactly that many bytes of body — and start the next section at the byte after them, never where the body happened to stop parsing
  - Keep the id of the last non-custom section you saw and refuse anything whose id is not strictly larger; that one check catches both duplicates and out-of-order sections
  - The data count section has id 12 but belongs before the code section, so treat it as the exception to ascending order rather than sorting by id
  - An id above 12 is a malformed module, not something to skip: only id 0, the custom section, may carry bytes you do not understand
- [ ] **Stage 04** — Custom sections and the name section **[ext]** (`src/stages/s04_custom_sections.rs`, 7 tests)
  - Id 0 is a custom section: read its size, skip exactly that many bytes, and go back to the loop without touching the last-section-id check that orders the others
  - It is legal before the first section, after the last one, in every gap between two, and repeated under a name you have already seen — none of that is an error to report
  - The `name` section is debug information for stack traces; whatever is in it, the module computes the same thing, so never let it reach validation
  - Skipping is not the same as not reading: the section's size and its UTF-8 name are still part of the frame, so a wrong size or a name that is not UTF-8 is a malformed module
- [ ] **Stage 05** — Truncated modules and sizes that lie (`src/stages/s05_truncated_modules.rs`, 8 tests)
  - Make every read of the module bounds-checked and make the failure a value you return, not a panic: a short file is ordinary input, not a bug in your program
  - A section's size and a function body's size are promises about bytes that may not be there — compare them against what is left of the file before you slice
  - A body's size is also its end: parse the instructions inside that window and refuse a body whose `end` falls outside it, rather than reading on into the next one
  - The code section's vector count and the function section's must agree, and both must agree with how many bodies the file actually holds
- [ ] **Stage 06** — Minimal modules and the export section (`src/stages/s06_minimal_modules.rs`, 7 tests)
  - Every section is optional: the eight-byte header alone is a valid module, so build your decoder around a loop that may run zero times rather than around a list of sections you require
  - An export is a name, a one-byte kind (0 func, 1 table, 2 memory, 3 global) and an index into that kind's index space — and the same index may be exported under as many names as you like
  - Export names are arbitrary UTF-8, including the empty string, so store them as bytes you validate rather than as an identifier you parse
  - Two exports with the same name is a validation error, not a decoding one: the bytes decode, the module is still refused, and nothing runs

## B. Validation & type checking

- [ ] **Stage 07** — Function bodies: arity and result types (`src/stages/s07_function_bodies.rs`, 8 tests)
  - Type-check every body against its declared signature before you execute anything: at the body's final `end` the stack must hold exactly the result types, in order
  - Falling off the end of a `() -> i32` body with an empty stack is a validation error, not an implicit zero — and a leftover value in a `() -> ()` body is one too
  - The function section and the code section are two parallel vectors: entry i of one describes entry i of the other, so different lengths make the module malformed
  - A function's local index space is its parameters first and then its declared locals, which come in runs of a count and a type — `3 i32` is three locals, not three bytes
- [ ] **Stage 08** — Operand stack underflow and leftovers (`src/stages/s08_stack_discipline.rs`, 7 tests)
  - Validate with a stack of types, not values: push the result types of each instruction and pop the operand types it needs, refusing the module the moment a pop finds nothing
  - Every instruction that takes operands can underflow — `drop`, `local.set`, `if`'s condition and a `call`'s arguments as much as `i32.add`
  - A `return` in the middle of a body must supply the function's results there and then; it does not inherit them from the end of the body
  - Check the types as well as the depth: `i32.add` on an i64 is as wrong as `i32.add` on an empty stack, and a stack that is merely the right height is not the right stack
- [ ] **Stage 09** — Unreachable code and the polymorphic stack **[ext]** (`src/stages/s09_unreachable_typing.rs`, 7 tests)
  - Give each block frame an `unreachable` flag; `unreachable`, `br`, `br_table` and `return` set it, and the block's `end` clears it
  - While the flag is set, popping an operand from an empty frame succeeds and yields whatever type was asked for — that is what makes `unreachable; i32.add` valid
  - It supplies types, it does not swallow them: a concrete value pushed inside unreachable code still has to match the block's result type at the `end`
  - Decode unreachable code anyway: an undefined opcode or an index that names nothing is a rejection even when the instruction can never run
- [ ] **Stage 10** — Block, loop and if result types (`src/stages/s10_block_types.rs`, 7 tests)
  - Read a block type as a function type: 0x40 means `() -> ()`, a value type byte means `() -> (that type)`, and anything else is a signed LEB128 naming a type in the type section
  - Push a frame at `block`, `loop` and `if` recording the stack height and the block's results, and at `end` check that exactly those results sit above that height
  - An `if` with a result type and no `else` cannot validate: the false path produces nothing, so a missing `else` is an error rather than an empty one
  - A `loop`'s label carries its parameter types, not its results — `br` to a loop is a jump back to the top, and the values it carries start the next iteration
- [ ] **Stage 11** — Unknown indices and immutable globals (`src/stages/s11_unknown_indices.rs`, 8 tests)
  - Every index in a body is checked against the size of its space before the body runs: functions, types, locals, globals, tables and memories are all dense and zero-based
  - A label index is a depth, not a declaration: count it against the frames you are holding, remembering that the function body is itself the outermost frame
  - A load or a store names memory 0 implicitly, so a module with no memory section cannot hold one at all
  - A global carries a mutability flag next to its type, and `global.set` on a global whose flag is 0 is a validation error, not a trap
- [ ] **Stage 12** — Validation runs before anything executes (`src/stages/s12_validation_before_execution.rs`, 7 tests)
  - Validate the whole module — every function body, not only the ones that are reachable — before you instantiate it or run its start function
  - A refused module must produce no side effects at all: nothing on stdout, no memory initialised, no start function called
  - Instantiation is a separate phase after validation: an active data or element segment that does not fit is a trap at instantiation, and the start function still never runs
  - Every label of a `br_table` has to carry the same result types, because one instruction has to be able to jump to any of them

## C. Numeric instructions & traps

- [ ] **Stage 13** — i32 arithmetic and its edge values (`src/stages/s13_i32_arithmetic.rs`, 9 tests)
  - i32.add, i32.sub and i32.mul are arithmetic modulo 2^32: compute in a wider type and mask, or use your language's wrapping operators — never the ones that panic on overflow
  - None of the three ever traps, so i32.mul of -2147483648 by -1 is -2147483648 again; only div_s and rem_s have an overflow trap, and that is the next stage but one
  - The operands come off the stack right to left: the value pushed first is the left operand, so `i32.const 10, i32.const 3, i32.sub` is 7 and not -7
  - and, or and xor operate on all thirty-two bits including the sign bit; keep the value in an unsigned 32-bit register internally and only reinterpret it as signed when you print it
- [ ] **Stage 14** — i64 arithmetic, wrapping and extension (`src/stages/s14_i64_arithmetic.rs`, 9 tests)
  - Keep i64 values in a real 64-bit register: a multiply done at thirty-two bits gives 0 for 65536 * 65536 where the answer is 4294967296
  - i32.wrap_i64 keeps the low thirty-two bits and drops the rest, sign bit and all, so the i64 2147483648 becomes the i32 -2147483648
  - i64.extend_i32_s copies the argument's sign bit into the top half and i64.extend_i32_u writes zeros there; they differ for every negative i32, and -1 becomes -1 or 4294967295 accordingly
  - An i64 constant is a signed LEB128 up to ten bytes long, so i64::MIN and i64::MAX need the full ten and a decoder that stops at five will silently truncate them
- [ ] **Stage 15** — Division, remainder and the two integer traps (`src/stages/s15_division_traps.rs`, 8 tests)
  - div_s truncates towards zero, so -7 / 2 is -3; rem_s takes the sign of the dividend, so -7 % 2 is -1 and 7 % -2 is 1
  - Check the divisor for zero before dividing anything: all eight of div_s, div_u, rem_s and rem_u at both widths trap with 'integer divide by zero'
  - i32::MIN / -1 and i64::MIN / -1 trap with 'integer overflow' — but div_u of the same bit patterns is 0, and it must not trap
  - i32::MIN rem_s -1 is 0 and is NOT a trap, so do not compute the remainder by dividing first; most hardware divide instructions fault on exactly this pair
- [ ] **Stage 16** — Comparisons, shifts and rotates (`src/stages/s16_comparisons_and_shifts.rs`, 9 tests)
  - Every comparison returns an i32 that is 0 or 1 — never -1, never the operand — even when the operands are i64
  - The signed and unsigned comparisons differ only in how they read the top bit: -1 lt_s 1 is 1 and -1 lt_u 1 is 0
  - Mask the shift count: count & 31 for i32 and count & 63 for i64, so 1 shl 32 is 1 and 1 shl 33 is 2, and a negative count is its unsigned value masked the same way
  - shr_s copies the sign bit into the top and shr_u writes zeros there; shr_s is not division, because -3 shr_s 1 is -2 while -3 div_s 2 is -1
- [ ] **Stage 17** — clz, ctz, popcnt and sign extension **[ext]** (`src/stages/s17_bit_counting.rs`, 8 tests)
  - clz and ctz of 0 are the width of the type — 32 for i32 and 64 for i64 — and that is the case a host intrinsic most often leaves undefined, so handle it before you call one
  - i32.clz, i32.ctz and i32.popcnt return an i32; their i64 counterparts return an i64, so i64.popcnt of -1 is the i64 value 64
  - i32.extend8_s takes the low eight bits, reads them as a signed byte and widens that: 0x7f is 127, 0x80 is -128, and whatever sat in the high twenty-four bits is discarded first
  - i64.extend32_s is the same idea at thirty-two bits and is a different instruction from i64.extend_i32_s, which takes an i32 operand rather than an i64 one
- [ ] **Stage 18** — f32 and f64 arithmetic (`src/stages/s18_float_arithmetic.rs`, 9 tests)
  - f32.const and f64.const carry four and eight raw little-endian bytes, not a LEB128: read them with from_le_bytes and never sign-extend them
  - Do every f32 operation at f32 width — computing in f64 and rounding once at the end gives a different answer for add, sub, mul, div and sqrt, and double rounding is how it shows up
  - ceil, floor and trunc all return a float, not an integer, and ceil(-0.5) is -0: keep the sign bit rather than normalising every zero to +0
  - Dividing a float by zero is not a trap — it is +inf, -inf or NaN depending on the signs, and only the integer divisions trap
- [ ] **Stage 19** — NaN, signed zeros, infinities, min/max and nearest (`src/stages/s19_float_special_values.rs`, 9 tests)
  - f32.min and f32.max are not fmin and fmax: if either operand is a NaN the result is a NaN, and min(+0, -0) is -0 while max(+0, -0) is +0
  - f32.nearest rounds a tie to the even neighbour — 0.5 and 2.5 both become 2's neighbours 0 and 2 — so it is neither round() nor trunc(x + 0.5)
  - copysign takes the magnitude of its first operand and the sign bit of its second, whatever that second operand is: a negative zero and a negative NaN both make the result negative
  - Every comparison with a NaN is false except ne, which is true; keep that rule out of your min/max and sorting code
- [ ] **Stage 20** — Truncation traps, trunc_sat and reinterpret **[ext]** (`src/stages/s20_conversions.rs`, 9 tests)
  - i32.trunc_f32_s rounds towards zero first and then checks the range, so -1.9 becomes -1 and 2147483647.5 becomes 2147483647 — test the truncated value, not the float
  - A NaN traps with 'invalid conversion to integer'; an infinity and any out-of-range finite value trap with 'integer overflow'. They are two different traps and a learner's runtime usually conflates them
  - The unsigned truncations accept everything that truncates to zero, so -0.5 is fine and -1.0 is an overflow; do not reject on the sign bit
  - The 0xfc-prefixed trunc_sat family never traps: NaN gives 0, too large gives the type's maximum, too small its minimum — and reinterpret moves bits without touching them at all

## D. Control flow

- [ ] **Stage 21** — block, loop and br to every depth (`src/stages/s21_blocks_and_loops.rs`, 8 tests)
  - `br l` counts labels outward from the branch: 0 is the innermost enclosing block, loop or if, and the function body itself is the outermost label
  - Branching to a `block` continues after its `end`; branching to a `loop` jumps back to the instruction after the `loop`. That one difference is the whole of iteration
  - A `br` takes the label's arity off the top of the stack, throws the rest of the frame away, and makes everything after it in the same block unreachable
  - Falling off the end of a block or a loop is not a branch: control simply carries on, which is why a `loop` nobody branches back to runs exactly once
- [ ] **Stage 22** — if / else and br_if (`src/stages/s22_if_else.rs`, 8 tests)
  - `if` pops one i32 and runs its first body when that value is anything but zero; `br_if l` pops one i32 and branches to `l` on the same test
  - Truth is 'not zero', not 'equal to one': -1, 2 and INT_MIN are all true, and the condition is consumed either way
  - `if bt … else … end` is a single block with two bodies, so both arms must leave exactly what the block type promises — an `if (result i32)` with no `else` cannot validate
  - A `br_if` that does not branch leaves everything except the condition where it was, so the value it would have carried is still on the stack
- [ ] **Stage 23** — br_table, its default and a runtime index (`src/stages/s23_br_table.rs`, 8 tests)
  - `br_table l0 l1 … ln ld` pops one i32: if it is less than the number of labels it branches to that label, and otherwise to the default `ld`
  - The index is **unsigned**, so -1 is 4294967295 and takes the default. An out-of-range index is a runtime decision, never a trap and never a validation error
  - Every label in the table, the default included, must have the same arity, and `br_table` carries that many values off the stack exactly as `br` does
  - The entries are relative depths like any other branch target, so the same label may appear more than once and the table need not be in any order
- [ ] **Stage 24** — return, unreachable, nop and drop (`src/stages/s24_return_and_unreachable.rs`, 7 tests)
  - `return` is a branch to the function's own label: it takes the result types off the top of the stack and discards whatever is underneath, from any depth
  - `unreachable` traps with the canonical reason 'unreachable'. The process exits non-zero, stderr says so, and stdout stays empty because the call never produced a result
  - Unreachable code still has to type-check, so `i32.const 7  return  unreachable` validates and returns 7 — refusing to decode what follows a `return` is wrong
  - `drop` pops exactly one value whatever its type, and `nop` does nothing at all; neither may be folded away into a type error
- [ ] **Stage 25** — select, typed and untyped (`src/stages/s25_select.rs`, 7 tests)
  - `select` pops the condition, then the second operand, then the first, and yields the **first** operand when the condition is non-zero
  - Both operands are already on the stack when it runs, so nothing is skipped: a trap in the operand that loses still happens
  - The two operands must have the same type, and that type is the result type — there is no promotion and no mixing
  - The untyped 0x1b form only accepts numeric operands; a funcref or externref needs the typed 0x1c form, which writes the type out
- [ ] **Stage 26** — Calls, recursion and call stack exhausted (`src/stages/s26_calls_and_recursion.rs`, 7 tests)
  - `call n` names a function index — imports first, then the module's own functions — and pops one argument per parameter, the first parameter being the value pushed first
  - The index space is complete before any body runs, so a function may call one defined later, may call itself, and two functions may call each other
  - The callee gets a fresh frame: the arguments become locals 0..n-1, the declared locals after them start at zero, and the operand stack starts empty
  - Count the call depth yourself and trap with 'call stack exhausted' when your own limit is reached; running out of the host's stack and dying is not a trap
- [ ] **Stage 27** — Multi-value blocks: results and parameters **[ext]** (`src/stages/s27_multi_value.rs`, 7 tests)
  - A block type that is neither 0x40 nor a value type is a **signed** LEB128 index into the type section, which gives the block both parameters and several results
  - A block's parameters are popped off the enclosing stack when it is entered; the values below them are untouchable until the block's `end`
  - A `loop`'s label arity is its **parameter** count, not its result count: a `br` back to a loop carries the next iteration's parameters
  - A function with several results returns them in order, and a runtime prints one per line

## E. Memory & bulk operations

- [ ] **Stage 28** — Loads and stores: every width and signedness (`src/stages/s28_loads_and_stores.rs`, 9 tests)
  - A load and a store both take a memory immediate — an alignment hint and a static offset — before they touch the stack; decode both even when you ignore the alignment
  - The narrow loads come in pairs: `load8_s` sign-extends the byte into the result type, `load8_u` zero-extends it, and 0xff is -1 or 255 depending on which one you wrote
  - A narrow store truncates: `i32.store8` writes the low byte of the operand and nothing else, so the three bytes after it must still hold whatever they held before
  - Memory starts as a page of zeroes, and a store followed by a load of the same address and width must give back exactly what went in — floats included, bit for bit
- [ ] **Stage 29** — offset, align and little-endian byte order (`src/stages/s29_offset_and_alignment.rs`, 8 tests)
  - The effective address is the dynamic operand plus the static `offset` immediate, added as unsigned 33-bit arithmetic — compute it in a u64 so a huge offset traps instead of wrapping back into the page
  - `align` is only a hint: it must never change the result and never trap, but a declared alignment larger than the width's natural one makes the module invalid, so check it while validating
  - The alignment immediate is a log2: 0, 1, 2, 3 stand for 1, 2, 4 and 8 bytes, and the natural alignment of a load is its width in bytes
  - Memory is little-endian for every type: the low byte of an i64 goes at the lowest address, and reading it back with eight `i32.load8_u` must give the bytes in that order
- [ ] **Stage 30** — memory.size and memory.grow (`src/stages/s30_memory_size_and_grow.rs`, 8 tests)
  - `memory.size` answers in pages of 65 536 bytes and starts at the memory's declared minimum, so divide by the page size rather than returning a byte count
  - `memory.grow n` returns the size *before* the growth; only the failure case returns -1, and -1 is a value, not a trap, so execution continues with it on the stack
  - A failed grow must leave everything alone: the same size, the same bytes, no partial allocation — write the new size only once you know it fits
  - Two limits can refuse a grow: the declared maximum, and the 65 536-page ceiling of a 32-bit memory that applies even when no maximum is declared
- [ ] **Stage 31** — Out-of-bounds memory traps at the byte boundary (`src/stages/s31_memory_bounds.rs`, 8 tests)
  - Bound the whole access, not its first byte: an access at address `a` of `w` bytes is in bounds when `a + w <= size_in_bytes`, computed wide enough not to overflow
  - That makes the last legal address width-dependent — 65 535 for one byte, 65 532 for four, 65 528 for eight on a one-page memory — and one past it must trap
  - Addresses are unsigned: `i32.const -1` means 4 294 967 295, so it is far outside the memory rather than one byte back from the end
  - The bound follows the current size, so recompute it after every `memory.grow` rather than caching the limit at instantiation
- [ ] **Stage 32** — Active and passive data segments (`src/stages/s32_data_segments.rs`, 7 tests)
  - Copy every active segment into its memory at instantiation, in the order the section lists them, before the start function runs and before any export is callable
  - A passive segment is copied nowhere: keep its bytes in the instance for `memory.init` and leave the memory as it was
  - A segment that does not fit is an instantiation trap, not a validation error — the module validates, then instantiation fails with `out of bounds memory access` and nothing runs
  - The offset is a constant expression, so it may be a `global.get` of an immutable global rather than a literal, and its value is only known once the instance is being built
- [ ] **Stage 33** — memory.init and data.drop **[ext]** (`src/stages/s33_memory_init.rs`, 7 tests)
  - `memory.init seg` pops length, then source offset, then destination — the operands are pushed in the order destination, source, length, so pop them the other way round
  - Check both ranges before copying anything: `d + n` against the memory size and `s + n` against the segment length, in arithmetic wide enough that a huge length cannot wrap
  - Length 0 is legal wherever both ends are in range, which includes the end of memory and the end of the segment; it copies nothing and must not trap
  - `data.drop` replaces the segment with an empty one rather than setting a flag: dropping twice is fine, and a later `memory.init` of length 0 from it is fine too
- [ ] **Stage 34** — memory.copy and memory.fill **[ext]** (`src/stages/s34_memory_copy_fill.rs`, 8 tests)
  - `memory.copy` must behave like `memmove`: when the ranges overlap, copy backwards if the destination is above the source and forwards if it is below, or the result is scrambled one way round
  - Its operands are pushed destination, source, length, so they come off the stack length first — the same order `memory.init` uses
  - `memory.fill` uses only the low eight bits of its value operand: 0x1ff fills with 0xff and 0x100 fills with zeroes
  - Check every bound before writing anything, and treat length 0 as legal wherever the addresses are in range — a copy of nothing at the end of memory must not trap

## F. Tables, globals, indirect calls

- [ ] **Stage 35** — Globals: mutable, immutable and imported (`src/stages/s35_globals.rs`, 8 tests)
  - A global section entry is three things: a value type byte, a one-byte mutability flag (00 const, 01 mut), and a constant expression ending in 0x0b
  - Evaluate the initialisers at instantiation, in order, into a per-instance vector of values; `global.get n` and `global.set n` then index that vector
  - `global.set` naming a global whose flag is 00 is a validation error, not a runtime trap — refuse the module before a single instruction runs
  - Imported globals come first in the global index space, exactly as imported functions come first in the function index space
- [ ] **Stage 36** — call_indirect and its three traps (`src/stages/s36_call_indirect.rs`, 8 tests)
  - `call_indirect` is 0x11 followed by two indices: the expected type, then the table — the table index is not optional padding, it selects which table to look in
  - Pop the index as an *unsigned* i32, so -1 means 4294967295 and is out of range, not one before the end
  - Keep the three failures apart: past the end of the table, a null slot, and a slot whose function has another type are three different traps
  - Compare the callee's type with the immediate structurally — same parameters, same results — never by comparing type indices
- [ ] **Stage 37** — Tables: get, set, size, grow, fill, copy **[ext]** (`src/stages/s37_table_operations.rs`, 8 tests)
  - `table.get` and `table.set` are plain opcodes (0x25, 0x26) with a table index; `size`, `grow`, `fill`, `copy` and `init` are 0xfc-prefixed and carry theirs in the second operand
  - `table.grow` returns the size before the growth, or -1 when it refuses, and a refused growth must leave the table untouched
  - `table.copy` may overlap in either direction: copy as though the whole source range were read before the first slot is written
  - A module may declare more than one table, so carry the table index through every one of these instructions instead of assuming table 0
- [ ] **Stage 38** — Element segments: active, passive, declarative **[ext]** (`src/stages/s38_element_segments.rs`, 7 tests)
  - The first byte of a segment is a bitfield, not an enum: bit 0 says passive-or-declarative, bit 1 says 'an explicit table index or element type follows', bit 2 says 'element expressions rather than function indices'
  - Apply active segments in order at instantiation, before the start function runs; a passive one waits for `table.init` and a declarative one is never applied at all
  - An active segment that does not fit its table is an instantiation *trap*, not a validation error — the module must decode and validate first
  - `elem.drop` makes the segment empty rather than removing it, so a later `table.init` of a non-zero length is out of bounds
- [ ] **Stage 39** — References and the start section **[ext]** (`src/stages/s39_references_and_start.rs`, 7 tests)
  - `ref.null t` (0xd0) carries the reference type as an immediate, `ref.is_null` (0xd1) takes any reference, and `ref.func n` (0xd2) names a function
  - Collect the set of function indices that appear anywhere outside a function body — exports, the start section, global initialisers, every element segment — and refuse a `ref.func` naming anything else
  - Run the start function at the end of instantiation: after the globals, after every active segment, and before the first export is callable
  - A start function that traps aborts instantiation — there is no instance afterwards, so no export can be called and nothing reaches stdout

## G. WASI preview1

- [ ] **Stage 40** — WASI: fd_write to stdout and stderr **[ext]** (`src/stages/s40_wasi_fd_write.rs`, 8 tests)
  - Import wasi_snapshot_preview1::fd_write with the signature (i32 i32 i32 i32) -> i32, and export your memory as `memory`: the host reads the buffers out of it
  - The third argument counts iovecs, not bytes; each one is {buf: i32, len: i32}, eight bytes, and they go into the stream in order
  - Store the number of bytes you really wrote at the fourth argument and return errno 0 — writing fewer than asked is allowed, and the caller is expected to loop
  - stdout is fd 1, stderr is fd 2, and a descriptor nobody opened is errno 8 (EBADF) with nothing written
- [ ] **Stage 41** — WASI: args, environ and proc_exit **[ext]** (`src/stages/s41_wasi_args_and_exit.rs`, 8 tests)
  - args_sizes_get(argc_out, buf_size_out) writes two i32s: how many arguments there are, and how many bytes args_get will need for all of them including one NUL each
  - argv[0] is the program name — the basename of the module path the runtime was given — so argc is one more than the number of arguments after the module on the command line
  - args_get(argv, argv_buf) writes the strings back to back into argv_buf, each terminated by a NUL, and one pointer into that buffer per argument into argv; pass the bytes through untouched, with no re-splitting on spaces and no re-encoding
  - proc_exit(code) ends the whole program there and then with that exit status: nothing after the call runs, no caller up the stack resumes, and its imported type is (i32) -> () because it never returns
- [ ] **Stage 42** — WASI: fd_read, clocks, randomness and missing imports **[ext]** (`src/stages/s42_wasi_io_and_imports.rs`, 8 tests)
  - fd_read(fd, iovs, iovs_len, nread) is fd_write backwards: fill the iovecs from the descriptor, store how many bytes you really moved at nread, and return errno 0 — end of input is nread 0 and errno 0, not an error
  - clock_time_get(id, precision, time_out) writes an i64 count of nanoseconds: id 0 is the realtime clock, counted from the Unix epoch, and id 1 the monotonic one, counted from an epoch you choose but never going backwards
  - random_get(buf, len) fills len bytes of the guest's memory and returns errno 0; a len of 0 is legal and must leave the memory exactly as it was
  - A module that imports a name you do not provide must be refused before anything runs: nothing on stdout, a non-zero exit, and a message naming the import — instantiation fails, it does not trap

## H. Robustness & scale

- [ ] **Stage 43** — Fuzz: 500 seeded mutations of valid modules **[ext]** (`src/stages/s43_fuzz.rs`, 6 tests)
  - Every length in a module is a claim, not a fact: check a section size, a vector count and a name length against the bytes you actually have before you index with them
  - A malformed module is input, never a reason to abort — refuse it with a message and a non-zero exit, and never print a byte on stdout, because a module that does not validate has not run
  - A uLEB128 that keeps setting the continuation bit must stop being read at the width of the field: ten 0xff bytes in a row is a malformed index, not a very large one
  - Run the fuzzer against your own runtime with --seed: the seed and the mutation index in a failure are enough to rebuild the exact bytes that broke it
- [ ] **Stage 44** — Scale: many functions, deep nesting, a big module **[ext]** (`src/stages/s44_scale.rs`, 6 tests)
  - Every section is a vector: read the count, then that many entries — decode them in a loop, because a decoder that recurses once per function dies at a thousand of them
  - Block nesting belongs on a stack you own, not on the host's call stack: several hundred levels of `block` must validate without growing a Rust frame per level
  - Locals are declared as (count, type) runs, so one run can ask for fifty thousand i32s; size the frame from the total the runs add up to, never from the number of runs
  - A multi-megabyte data segment is copied into memory once at instantiation — slice it straight out of the module bytes rather than building a Vec per byte
- [ ] **Stage 45** — Soak: compute, repetition and determinism **[ext]** (`src/stages/s45_soak.rs`, 6 tests)
  - A hundred million loop iterations must not allocate: the operand stack of a loop body returns to the same height every time round, so reuse the frame instead of pushing a new one
  - The same module with the same arguments must print byte-identical output every single time — determinism is the one thing a runtime may never trade for speed
  - Integer arithmetic wraps and is exact; float arithmetic is IEEE-754 to the letter, so summing the first ten million integers in f64 gives exactly 50000005000000 and nothing near it
  - Growing a memory to eight megabytes and touching every byte is a normal thing for a module to do — keep the pages in one allocation and index it, do not copy on grow more than you must
