# linktest stage plan

Tick a stage when `linktest --linker my_linker --stage N` is green. `linktest --list` reads
these boxes. Stages marked **[ext]** go beyond the core track (`--skip-ext` hides them).
All 42 stages are implemented — 341 tests, 87 worked examples — so no entry
says **(planned)**; each one names its source file and its test count. See README.md,
"Adding a stage".

Run one stage: `linktest --linker my_linker --stage 5` — everything so far: `--until 12` —
the lot: `--all`. Prove the suite itself: `linktest --linker gnu_ld --validate --all`.

Every stage carries 1-3 **worked examples**: a real relocatable object, annotated field by
field, next to the linker command line and the exact stdout and exit status the linked
program must produce. They live in the stage's own file and reach the site through
`catalog.json`. See README.md, "Examples".

The program under test is `./your_program.sh -o <out> [-e <entry>] [-L <dir>] [-l <name>]
<input.o|input.a>...` and its output is a statically linked, non-PIE ELF64 executable for
x86-64 Linux that the kernel runs directly. See README.md, "The contract your linker must
follow".


## A. Reading relocatable objects

- [ ] **Stage 01** — Link one object and run it (`src/stages/s01_link_one_object.rs`, 8 tests)
  - Read the 64-byte ELF header, then the section header table it points at, and keep the SHF_ALLOC sections
  - Give .text an address, write one PT_LOAD segment that covers it, and set e_entry to the address of _start
  - e_type is ET_EXEC (2), e_machine EM_X86_64 (62); the output has to be chmod 0755 or the kernel will not run it
  - Nothing here needs relocation yet: one object with no .rela sections is the whole job
- [ ] **Stage 02** — Reject what is not an x86-64 relocatable object (`src/stages/s02_reject_foreign_input.rs`, 9 tests)
  - Check e_ident first — the four magic bytes, EI_CLASS 2 (ELFCLASS64), EI_DATA 1 (little-endian) — then e_type ET_REL and e_machine EM_X86_64, and refuse anything else before reading a single section header
  - A refusal is an exit status, not a panic: write one line to stderr that names the offending input file, exit non-zero, and leave no runnable output behind
  - An executable and a shared object are ELF too, and a static linker that only understands relocatable objects has to say so rather than folding their headers into its own output
  - Validate every input before allocating anything, so that a bad third input does not leave a half-written executable in place of the old one
- [ ] **Stage 03** — Read the ELF header of an input (`src/stages/s03_read_the_elf_header.rs`, 8 tests)
  - Take e_shoff, e_shentsize and e_shnum out of the header and walk the section header table from there: the table is wherever e_shoff says, not at a fixed offset such as 0x40
  - EI_OSABI, EI_ABIVERSION, the seven reserved e_ident padding bytes and e_flags are all information an x86-64 static linker ignores; refusing them means refusing real compiler output
  - A relocatable object's e_entry and e_phoff are meaningless — the executable's entry comes from the entry symbol you resolve, and its program headers are ones you write yourself
  - The table is exactly e_shnum entries of e_shentsize bytes; whatever follows it in the file is not yours to read
- [ ] **Stage 04** — String tables: section and symbol names (`src/stages/s04_string_tables.rs`, 8 tests)
  - Section names come out of the string table e_shstrndx points at; symbol names come out of the one .symtab's sh_link points at — two tables, two indices, and neither is guaranteed to be the last section
  - A name is the NUL-terminated string at its offset: read to the NUL, never to a fixed maximum, and keep the whole thing
  - Compare whole names and nothing less: `strings` is a suffix of `.rodata.strings` and `counter` is a suffix of `mycounter`, and they are four different names
  - Assembler and compiler output is full of dots, dollars and digits in symbol names; a name is bytes, not an identifier
- [ ] **Stage 05** — A section header table that lies **[ext]** (`src/stages/s05_section_header_table.rs`, 8 tests)
  - Bounds-check before you dereference: e_shoff + e_shnum * e_shentsize must fit in the file, and so must sh_offset + sh_size of every section that occupies file space
  - An ELF64 section header is exactly 64 bytes, so an e_shentsize that says anything else means this is not a file you can read
  - Check that .symtab's sh_link is in range and really names a SHT_STRTAB before reading any symbol name through it
  - Refusing is right and tolerating can be right, but crashing, hanging and silently writing a broken executable never are
- [ ] **Stage 06** — Truncated and malformed objects **[ext]** (`src/stages/s06_truncated_objects.rs`, 8 tests)
  - Read the whole input into memory once and check every offset and length against that buffer before using it — a truncated object is exactly a file whose offsets point past its end
  - The three places a truncation shows up are the 64-byte header, the section contents and the section header table; a bounds check in one of the three is a bounds check in none of them
  - Refusing has to be an exit status with a message on stderr, not an index panic and not a silent zero-filled read
  - If you have already created the output file when you discover the problem, remove it or never mark it executable: a failed link must not leave something runnable behind
- [ ] **Stage 07** — Odd but legal objects **[ext]** (`src/stages/s07_odd_but_legal_objects.rs`, 8 tests)
  - A section type you do not recognise is not an error: if it is not SHF_ALLOC it contributes nothing to the image, so skip it and say nothing
  - Nothing says .rela.text comes after .text, or that .symtab is last: index the section header table by number, never walk it expecting an order
  - sh_addralign 0 and 1 both mean 'no constraint', and an empty SHF_ALLOC section is legal — give it an address and move on
  - An input that contributes no bytes at all still contributes its symbols, and an input with no symbols at all is still a legal object

## B. Emitting a runnable executable

- [ ] **Stage 08** — The ELF header of the executable (`src/stages/s08_executable_header.rs`, 8 tests)
  - Write `e_type = ET_EXEC` (2), not `ET_DYN`: this track links non-PIE static executables, so the addresses in the file are the addresses the program runs at
  - `e_ehsize` is 64, `e_phentsize` 56 and `e_shentsize` 64 — these are structure sizes, not choices, and a kernel that disagrees with them refuses the file
  - `e_ident[7..16]` is padding and must be zero, `e_version` and `e_ident[6]` are both 1, and `e_flags` is 0 on x86-64 — there are no processor flags to set
  - Emit a `.symtab` (and its `.strtab`) naming the globals you defined: the suite looks `_start` up there, and so does every debugger and every `nm`
- [ ] **Stage 09** — Program headers and PT_LOAD (`src/stages/s09_program_headers.rs`, 8 tests)
  - The kernel maps `PT_LOAD` segments and ignores section headers entirely, so every allocated section has to end up inside one of them
  - Group the output sections by permission and emit one `PT_LOAD` per group; one segment per section also works, it is just wasteful of pages
  - `p_filesz` is what is read from the file and `p_memsz` what is mapped, so `p_filesz <= p_memsz` always, and the difference is `.bss`
  - This is a static link: no `PT_INTERP` and no `PT_DYNAMIC`, because there is no loader to name and nothing for it to do
- [ ] **Stage 10** — The entry point (`src/stages/s10_entry_point.rs`, 8 tests)
  - `e_entry` is `symbol_address("_start")` after resolution — look it up, never assume it is the base of `.text` or the first byte of the first input
  - `_start` can live in any input, at any offset in any section, with other symbols in front of it; the address is section base plus `st_value`
  - `-e name` replaces the symbol that is looked up, and nothing else: the layout, the sections and every other address stay exactly as they were
  - The kernel jumps straight to `e_entry` with no return address on the stack, so `_start` must never `ret` — it exits with a syscall
- [ ] **Stage 11** — Segment permissions and W^X (`src/stages/s11_segment_permissions.rs`, 8 tests)
  - Turn `SHF_EXECINSTR` into `PF_X` and `SHF_WRITE` into `PF_W`, always keeping `PF_R`: a segment nothing can read is a segment nothing can use
  - Group sections by the permission set they end up with and give each group its own `PT_LOAD`, so that `.text` never shares a segment with `.data`
  - No `PT_LOAD` may carry both `PF_W` and `PF_X` — merging read-only data into the text segment is fine, merging `.data` into it is not
  - Permissions are enforced by the MMU at page granularity, so two sections that share a page share its permissions whatever the section headers say
- [ ] **Stage 12** — Page alignment and the mmap congruence (`src/stages/s12_page_alignment.rs`, 8 tests)
  - For every `PT_LOAD`, `p_offset % 0x1000` must equal `p_vaddr % 0x1000`; the easy way to guarantee it is to start each segment on a page boundary in both
  - `p_align` is the alignment the segment was laid out for: a power of two, and at least the page size for anything the kernel maps
  - Rounding the address up to a page and the offset up to a page independently is the classic bug — round one of them and then derive the other
  - A section whose `sh_addralign` is larger than a page (a 64 KiB table, say) raises the segment's `p_align` too, and the congruence still has to hold
- [ ] **Stage 13** — .bss: memory without file bytes (`src/stages/s13_bss.rs`, 8 tests)
  - An `SHT_NOBITS` input section has a size but no bytes: advance the output address by its size and copy nothing, because `sh_offset` points at other sections' data
  - The segment that covers `.bss` gets `p_memsz` larger than `p_filesz`; the difference is what the kernel zero-fills, and it is the whole trick
  - Put every `SHT_NOBITS` section last inside its segment — file bytes have to be contiguous from `p_offset`, so nothing with real bytes may follow the hole
  - `.bss` must land in a writable, non-executable segment, and a megabyte of it must not add a megabyte to the output file
- [ ] **Stage 14** — Concatenating input sections (`src/stages/s14_section_concatenation.rs`, 8 tests)
  - Collect the input sections by output name, then place them one after another, remembering for each contribution the offset it was given
  - Keep the command-line order: every linker lays contributions down in the order the inputs were named, and programs — and debuggers — rely on it
  - Round the running address up to each contribution's own `sh_addralign` before placing it, not once for the whole output section
  - A symbol's output address is the address its contribution got plus its input `st_value`; get this wrong by one contribution and every relocation is wrong
- [ ] **Stage 15** — Alignment and empty sections **[ext]** (`src/stages/s15_alignment_and_empty_sections.rs`, 8 tests)
  - Round the running address up with `(addr + align - 1) & !(align - 1)`, and treat an `sh_addralign` of 0 or 1 as no constraint rather than as a divisor
  - A zero-size section contributes nothing: do not let it advance the address, and do not let it create an output section out of nothing
  - An alignment larger than a page raises the segment's `p_align`, and the page congruence still has to hold — pad the file to match, or pick the offset from the address
  - Symbols in an empty section are legal; they just all have the same address, and they must not end up pointing outside every segment

## C. Symbol resolution

- [ ] **Stage 16** — A global defined in another object (`src/stages/s16_global_across_objects.rs`, 8 tests)
  - Make one pass over every input first, recording each STB_GLOBAL definition (name, its object, its section and st_value) before you try to patch anything
  - A relocation's S is the *output* address of the symbol it names: the address the defining input section was given, plus the symbol's st_value inside it
  - Undefined entries (st_shndx == SHN_UNDEF) are references, not definitions — an object mentioning a name never means it provides it
  - Command-line order must not change the answer here: with only objects on the line, every one of them is loaded, so `a.o b.o` and `b.o a.o` link the same program
- [ ] **Stage 17** — An undefined symbol is an error (`src/stages/s17_undefined_symbol.rs`, 8 tests)
  - After collecting every definition, walk every relocation: if the symbol it names is still SHN_UNDEF and not weak, that is an error, not a zero
  - The message has to contain the symbol's name — a learner debugging a 200-object link needs the name far more than they need your wording
  - Report every unresolved name you find, not just the first, and exit non-zero
  - Delete (or never create) the output file on failure: a stale a.out that a build system thinks is fresh is worse than no output at all
- [ ] **Stage 18** — A duplicate definition is an error (`src/stages/s18_duplicate_definition.rs`, 8 tests)
  - Keep one table keyed by name; inserting a strong definition where a strong definition already sits is the error, and the message must carry the name
  - Only *definitions* collide — an SHN_UNDEF entry for the same name is a reference and must never trip the check
  - A strong definition and a weak one are legal: the strong one wins silently, and a second weak one is not an error either
  - The same file named twice on the command line is two inputs, not one; do not deduplicate by path to make the error go away
- [ ] **Stage 19** — Local symbols do not clash (`src/stages/s19_local_symbols.rs`, 7 tests)
  - Resolve a relocation against a local symbol inside the object that owns it: take the symbol index straight out of that object's own .symtab and never consult the global table
  - The binding half of st_info decides everything; sh_info of .symtab tells you where the locals stop, and locals always come first
  - A local never enters the global table, so two objects may each have a local `helper` and a third object's reference to `helper` must still find the global one — or fail as undefined if there is none
  - Section symbols (STT_SECTION, empty name) are locals too: `.rodata` in five objects means five distinct section symbols with five distinct addresses
- [ ] **Stage 20** — Weak definitions and weak references (`src/stages/s20_weak_symbols.rs`, 8 tests)
  - A strong definition always beats a weak one, whichever input it came from and in whatever order the objects are named — the winner must not depend on the command line
  - A weak definition is still a definition: with no strong one in the link it is used, and it never counts as a duplicate
  - An unresolved weak *reference* is not an error; compute the relocation with S = 0 and let the program test for it
  - Two weak definitions of one name are not an error either; pick one deterministically (GNU ld takes the first it sees) and move on
- [ ] **Stage 21** — Common symbols (SHN_COMMON) **[ext]** (`src/stages/s21_common_symbols.rs`, 7 tests)
  - For a symbol whose st_shndx is SHN_COMMON, st_value is the required alignment and st_size the required size — neither means what it means for a normal symbol
  - Merge every common of one name into a single .bss allocation whose size is the largest st_size and whose alignment is the largest st_value, then give every reference that one address
  - Commons never collide with each other, but any real definition — .data, .bss, .text, strong or weak — takes precedence and the commons are dropped
  - The allocation is zero-filled and writable: it belongs in a SHT_NOBITS section inside a read-write PT_LOAD, and costs no bytes in the file
- [ ] **Stage 22** — Visibility, absolute and section symbols **[ext]** (`src/stages/s22_visibility_and_absolute.rs`, 8 tests)
  - st_other's low two bits are the visibility; in a static link with no dynamic table they change nothing about resolution, so read them, keep them, and never refuse a link over them
  - A symbol whose st_shndx is SHN_ABS already has its final value: S is st_value itself, with no section address added — this is the one case where relocating is the bug
  - A relocation may name a STT_SECTION symbol instead of a real one; S is then the output address the section was given, and the addend picks the byte inside it
  - STT_FILE entries are local, absolute and carry a source file name; skip them rather than treating them as definitions of anything
- [ ] **Stage 23** — The entry symbol and -e (`src/stages/s23_entry_symbol.rs`, 8 tests)
  - Resolve the entry name through the same global symbol table as everything else, then put its final address — section address plus st_value — in e_entry
  - `-e name` and `--entry=name` are the same option; with neither, the name is `_start`, and it may be defined in any input, not just the first
  - The entry symbol need not sit at offset 0 of its section: st_value is part of the answer and dropping it points e_entry at whatever happens to be first
  - Decide what to do when the entry name is undefined and say so — GNU ld warns and carries on with a defaulted address, which is friendly but easy to miss; failing outright is also defensible

## D. Relocations

- [ ] **Stage 24** — R_X86_64_PC32 and R_X86_64_PLT32 (`src/stages/s24_pc32_and_plt32.rs`, 8 tests)
  - Walk every SHT_RELA section of every input and compute S + A - P for each entry: S is the final address of the symbol, A is r_addend, P is the final address of the four-byte field being patched
  - P is the address of the field, not of the instruction — the -4 the assembler already folded into the addend is what turns that into the end of the instruction the processor measures displacements from
  - Treat R_X86_64_PLT32 exactly like R_X86_64_PC32: with no PLT to build, a static link resolves both straight to the symbol, and a linker that handles only one of them dies on the first real compiler output it sees
  - Write the result into the output image as a little-endian signed 32-bit value, never back into the input object, and check that it fits before storing it
- [ ] **Stage 25** — R_X86_64_64 (`src/stages/s25_absolute_64.rs`, 8 tests)
  - R_X86_64_64 is the easy one: store the 64-bit value S + A, little-endian, into the eight bytes at the relocation's site — no program counter, no truncation, no range check
  - The site is in a data section, not in .text, so the patch loop has to run over every relocated section of every input and not just over the code
  - Patch the bytes of the output image after the section has been copied into it: patching the input object's buffer and copying afterwards works only until two inputs share a section name
  - A relocation inside .rodata is still applied — read-only is a property of the final mapping, not a reason to leave the word zero
- [ ] **Stage 26** — R_X86_64_32 and R_X86_64_32S (`src/stages/s26_absolute_32.rs`, 8 tests)
  - Both kinds compute S + A and store the low 32 bits; the only difference is the range they are allowed to hold, so share the arithmetic and split only the check
  - These are the relocations that make the small code model work: the whole image is below 4 GB, so a zero-extended 32-bit immediate is a complete pointer and the program can dereference it as one
  - An R_X86_64_32 can sit in a data section as easily as in an instruction — a four-byte word holding an address is the same relocation as `mov eax, $sym`
  - Do not sign-extend an R_X86_64_32 or zero-extend an R_X86_64_32S when checking the range: 0xffffffff is legal for the first and out of range for the second
- [ ] **Stage 27** — Relocation overflow is a hard error **[ext]** (`src/stages/s27_relocation_overflow.rs`, 8 tests)
  - After computing the value of a relocation, check it fits the field before you store it: R_X86_64_32 needs 0 <= v <= 0xffffffff, R_X86_64_32S and R_X86_64_PC32 need -0x80000000 <= v <= 0x7fffffff
  - The check is on S + A, not on S: an addend can carry a perfectly ordinary symbol past the end of the range
  - Run the check over every relocated section, data included — an overflowing pointer word in .data is as fatal as one in .text
  - Refusing means exiting non-zero with the symbol's name on stderr and leaving no output file behind; a truncated value produces a binary that fails far from the mistake
- [ ] **Stage 28** — Addends, positive and negative (`src/stages/s28_addends.rs`, 8 tests)
  - r_addend is a signed 64-bit field of the relocation entry: read it as an i64, add it to S, and never look at what the field being patched already holds
  - In a SHT_RELA object the contents of the field are irrelevant — the assembler usually leaves zeros there, but a linker that adds to them instead of storing over them is only right by accident
  - The -4 on every RIP-relative relocation is an addend like any other; there is no special case for it, and adding a second -4 of your own doubles it
  - An addend may point outside the symbol it names — one past the end is normal and must not be rejected
- [ ] **Stage 29** — Relocations against section symbols **[ext]** (`src/stages/s29_section_symbols.rs`, 8 tests)
  - An STT_SECTION symbol's value is the address of that object's contribution to the output section, so resolve it to where you placed that input section — not to the start of the merged output section
  - The offset inside the section is carried entirely in r_addend, which is why these relocations almost always have a large addend and no name
  - Two inputs both contributing .rodata have two different section symbols with the same name; keying anything on the section's name instead of on the input section is how the second object's literals end up pointing into the first
  - A section symbol usually has an empty st_name, so a symbol table keyed by name needs somewhere else to put them
- [ ] **Stage 30** — References across sections and objects (`src/stages/s30_cross_section_references.rs`, 8 tests)
  - Give every input section its final address before applying a single relocation: the value of a symbol is its section's address plus st_value, and that is only knowable once layout is finished
  - Keep one map from (input object, section index) to the address that section landed at — it answers both 'where is this symbol' and 'where is this relocation site'
  - A relocation's site is in the section that owns the .rela, and its target can be in any section of any object; nothing about the arithmetic changes with the direction
  - Data can point at code and code at data: a function pointer in .data is an R_X86_64_64 whose symbol happens to be STT_FUNC, and nothing special is needed for it
- [ ] **Stage 31** — GOTPCREL and its relaxation **[ext]** (`src/stages/s31_gotpcrel.rs`, 8 tests)
  - The simple implementation is a real GOT: collect every symbol a GOTPCREL names, give each one an eight-byte slot in a section you allocate, fill the slot with S, and resolve the relocation to GOT_slot + A - P
  - The other implementation is relaxation: in a static link S is a constant, so `mov r64, [rip+GOT]` can become `lea r64, [rip+sym]` — same length, one fewer memory reference, no GOT
  - Whichever road you take, the register must end up holding the address of the symbol and not the contents of it; a GOT slot holds an address, so the load reads a pointer, not the object
  - Only relax when you are sure: R_X86_64_GOTPCRELX and R_X86_64_REX_GOTPCRELX exist precisely to tell the linker the instruction is safe to rewrite

## E. Archives and link order

- [ ] **Stage 32** — An archive on the command line (`src/stages/s32_archive_basics.rs`, 9 tests)
  - Recognise an archive by its first eight bytes, `!<arch>\n`, not by the `.a` on the file name — the name is a convention and `-l` will hand you paths you did not choose
  - A member header is 60 bytes of space-padded ASCII; the only two fields you need are the 16-byte name and the decimal size at offset 48, and every member starts on an even offset, so round the size up when you step to the next one
  - A name of `/<decimal>` is an offset into the `//` member, and a short name is written with a trailing `/` that is not part of the name — strip it
  - Once you have a member's bytes you are back in stage 01: it is an ordinary ET_REL object and the reader you already have parses it unchanged
- [ ] **Stage 33** — Only the members that are needed (`src/stages/s33_archive_member_selection.rs`, 9 tests)
  - Keep the set of still-undefined symbols as you go; when you reach an archive, load exactly the members that define one of them and nothing else
  - Loading a member adds its own undefined symbols to that set, so loop over the archive until a whole pass pulls nothing in — one pass is not enough when a member needs a member that sits earlier in the same file
  - A symbol that is already defined does not make a member needed, and neither does an undefined *weak* reference: a weak reference resolves to zero rather than dragging a definition in
  - Never validate a member you did not load — its duplicate symbols and its undefined references are somebody else's problem until the day it is needed
- [ ] **Stage 34** — Order on the command line is semantics (`src/stages/s34_link_order.rs`, 9 tests)
  - Walk the inputs strictly left to right and keep one set of still-undefined symbols; an archive reached before the reference that needs it simply has nothing to offer and is dropped
  - Do not go back: once an archive has been scanned and taken nothing, a reference made by a later input does not reopen it — `--start-group` is the opt-in for that, and it is stage 36
  - Two definitions of one symbol in two archives is not an error: the first archive on the command line supplies it and the second is simply never asked
  - Objects are concatenated in command-line order, so the first object's .text is the one that gets the lowest address — sorting inputs by name or by size changes every address in the output
- [ ] **Stage 35** — -L and -l (`src/stages/s35_library_search_path.rs`, 9 tests)
  - `-l <name>` means `lib<name>.a`, matched exactly — `libxy.a` is not a match for `-l x`, and the file name is built by wrapping the name, never by searching for it
  - Search the `-L` directories in the order they were given and take the first directory that has the file, not the last and not the best
  - When nothing matches, exit non-zero with a message that contains the library's name; `cannot find -lfoo` is the wording GNU ld uses and the name is the part that matters
  - Resolving `-l` to a path is all this option does: the archive it found is linked at the position the `-l` occupied, under the ordering rules of stage 34
- [ ] **Stage 36** — Groups and broken symbol indexes **[ext]** (`src/stages/s36_archive_edge_cases.rs`, 9 tests)
  - `--start-group`/`--end-group` bracket a set of archives that is rescanned as a whole until a pass pulls nothing in; the loop must terminate on the first barren pass, not on a fixed number of tries
  - Treat the `/` index as a hint you verify, not as the truth: after opening the member it points at, check that the member really does define the symbol before you count it as resolved
  - An archive with no `/` member at all is still a perfectly good archive — read each member's own symbol table instead
  - Every malformed archive here has to end in a diagnostic and a non-zero exit, never a panic, an infinite loop or a half-written output file

## F. Real programs, robustness, scale

- [ ] **Stage 37** — A six-object freestanding program (`src/stages/s37_multi_object_program.rs`, 8 tests)
  - Read every input before resolving anything: object six defines what object one referenced, and object one is read first
  - Concatenate the same-named sections in command-line order, then give each symbol its final address as output section base + input contribution offset + st_value
  - Apply relocations only once every address is fixed — a PLT32 call between two objects is just S + A - P as soon as both .text contributions have landed
  - The binary inherits argv and stdin from whoever runs it; a correct link touches neither, so the same program must behave identically however it is invoked
- [ ] **Stage 38** — Two hundred objects **[ext]** (`src/stages/s38_many_objects.rs`, 9 tests)
  - Put every global in one hash map keyed by name as the inputs are read, so resolution is one lookup per reference rather than a scan over every object
  - Concatenate each output section once, in input order, and remember every input contribution's base address — recomputing it per relocation is the quadratic trap
  - Two hundred inputs means two hundred open files; read each one once into memory and close it, or the link dies on the file-descriptor limit long before it dies on time
- [ ] **Stage 39** — A five-megabyte .rodata **[ext]** (`src/stages/s39_large_sections.rs`, 8 tests)
  - Keep every offset, address and size in a u64 and do the arithmetic with checked or saturating operations — five megabytes is where a u32 section offset first hurts
  - p_filesz and p_memsz are different numbers: a SHT_NOBITS section adds to the second and never to the first, so an eight-megabyte .bss must cost zero bytes on disk
  - The page congruence p_offset = p_vaddr (mod 4096) has to hold for a five-megabyte segment exactly as it does for a forty-byte one; pad the file, do not move the address
  - Honour every input section's sh_addralign when you concatenate, including the one whose contribution ends at an odd offset — the next section starts at the next multiple, not the next byte
- [ ] **Stage 40** — Fuzz: seeded mutations of valid objects **[ext]** (`src/stages/s40_fuzz.rs`, 8 tests)
  - Validate before you trust: every offset, size and index read out of an input has to be checked against the length of the file before it is used to slice it
  - Use checked arithmetic on everything that came from the file — sh_offset + sh_size overflowing a u64 is a one-line panic and a panic is a signal
  - Refusing an input is always allowed and accepting a harmless lie is always allowed; being killed, hanging, or exiting 0 with a broken output file never is
  - Never allocate a buffer of a size an input told you — sh_size can say sixteen exabytes, and the answer is a diagnostic, not a memory request
- [ ] **Stage 41** — Determinism **[ext]** (`src/stages/s41_determinism.rs`, 8 tests)
  - Iterate over ordered containers only: a hash map walked in whatever order it happens to be in is the single most common source of a non-reproducible linker
  - Zero every byte of padding you write — an uninitialised buffer is a different output every run and a security hole in the same breath
  - Nothing that is not in the inputs may reach the output: not the time, not the output file's name, not the path an input was read from, not a process id
  - Truncate the output file when you open it; a relink over a longer previous output must not leave the old tail behind
- [ ] **Stage 42** — readelf, nm and objdump agree **[ext]** (`src/stages/s42_toolchain_interop.rs`, 8 tests)
  - If readelf prints a warning about the file, fix the field it is complaining about — it is almost always sh_link, sh_entsize or an sh_offset that runs past the end
  - objdump and nm read the section header table, not the program headers: an executable that runs perfectly can still be unreadable to every tool if its section headers are wrong or missing
  - A .symtab is optional for running and essential for debugging; emit one with st_value set to each symbol's final address, sh_link pointing at the .strtab and sh_info equal to the index of the first non-local

---

42 stages, 341 tests, 15 stages marked `[ext]`. The suite self-check is
`linktest --linker gnu_ld --validate --all`; it must be all green, and `cargo test` proves
`catalog.json` and this file still match the stage registry.

## G. What real toolchains expect

Two things a static linker has to do before it can link a program a compiler produced, and
neither of them is a relocation: run the code that runs before `main`, and delete the code
nothing can reach.

- [ ] **Stage 43** — Init and fini arrays **[ext]** (`src/stages/s43_init_arrays.rs`, 9 tests)
  - `.init_array` is `SHT_INIT_ARRAY`, allocated and writable, and holds an array of 8-byte function pointers — concatenate the inputs' copies in command-line order
  - `__init_array_start` and `__init_array_end` come from no object file: the linker defines them at the two ends of the section it produced
  - When nothing has an `.init_array`, those two symbols must still resolve and must be equal, so a walker runs zero times instead of walking nothing
  - `.fini_array` is the same in every respect, with its own pair of symbols, and is walked backwards at exit
- [ ] **Stage 44** — --gc-sections: deleting what nothing reaches **[ext]** (`src/stages/s44_gc_sections.rs`, 9 tests)
  - Treat allocated sections as nodes and relocations as edges: mark everything reachable from the roots, then drop every section that was not marked
  - The entry point's section is a root, and so is `.init_array` — nothing references a constructor, so marking only from the entry deletes them all
  - Reachability is transitive and runs through data too: a function that references a string keeps the section holding the string
  - Without the flag nothing is dropped, so the same inputs must link both ways and differ only in what came out
