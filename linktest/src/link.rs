//! One link: the inputs on disk, the command line, the result, and the parsed output.

use crate::assert::{Failure, FailureKind};
use crate::elf::read::Elf;
use crate::exec;
use std::path::PathBuf;

/// A link waiting to be run.
///
/// The order of `object`/`archive`/`arg` calls is the order the arguments reach the linker,
/// because in a linker command line order is semantics: an archive before the object that
/// needs it resolves nothing.
#[derive(Debug, Clone, Default)]
pub struct Link {
    /// Files to write into the link's directory before running: `(relative path, bytes)`.
    pub files: Vec<(String, Vec<u8>)>,
    /// Everything after `-o <out>`, in order.
    pub args: Vec<String>,
    /// Name of the output file, relative to the link's directory.
    pub out: String,
    /// A label for the report (which link of the test this was).
    pub label: String,
}

impl Link {
    /// An empty link that writes `a.out`.
    pub fn new() -> Link {
        Link {
            files: Vec::new(),
            args: Vec::new(),
            out: "a.out".to_string(),
            label: String::new(),
        }
    }

    /// Write `name` into the link's directory and pass it as an input.
    pub fn object(mut self, name: &str, bytes: Vec<u8>) -> Link {
        self.files.push((name.to_string(), bytes));
        self.args.push(name.to_string());
        self
    }

    /// Write an archive into the link's directory and pass it as an input by path.
    pub fn archive(self, name: &str, bytes: Vec<u8>) -> Link {
        self.object(name, bytes)
    }

    /// Write a file without passing it on the command line — the member of a `-L` directory
    /// that `-l` is meant to find.
    pub fn write_file(mut self, path: &str, bytes: Vec<u8>) -> Link {
        self.files.push((path.to_string(), bytes));
        self
    }

    /// Append one raw argument.
    pub fn arg(mut self, a: &str) -> Link {
        self.args.push(a.to_string());
        self
    }

    /// Append several raw arguments.
    pub fn args<'s>(mut self, args: impl IntoIterator<Item = &'s str>) -> Link {
        self.args.extend(args.into_iter().map(str::to_string));
        self
    }

    /// `-e <symbol>`.
    pub fn entry(self, symbol: &str) -> Link {
        self.arg("-e").arg(symbol)
    }

    /// `--entry=<symbol>`, the long form.
    pub fn entry_long(self, symbol: &str) -> Link {
        self.arg(&format!("--entry={symbol}"))
    }

    /// `-L <dir>`.
    pub fn lib_dir(self, dir: &str) -> Link {
        self.arg("-L").arg(dir)
    }

    /// `-l <name>`, which means "look for `lib<name>.a` in the `-L` directories".
    pub fn lib(self, name: &str) -> Link {
        self.arg("-l").arg(name)
    }

    /// Name the output something other than `a.out`.
    pub fn out(mut self, name: &str) -> Link {
        self.out = name.to_string();
        self
    }

    /// Label this link in the report ("the second link", "the relink").
    pub fn label(mut self, label: &str) -> Link {
        self.label = label.to_string();
        self
    }

    /// The full argument list, `-o <out>` first.
    pub fn argv(&self) -> Vec<String> {
        let mut v = vec!["-o".to_string(), self.out.clone()];
        v.extend(self.args.iter().cloned());
        v
    }
}

/// What one invocation of the linker did.
#[derive(Debug, Clone)]
pub struct LinkRun {
    /// The directory the link ran in; the inputs and the output are there.
    pub dir: PathBuf,
    /// The process result.
    pub output: exec::Output,
    /// Where the output file was supposed to appear.
    pub out_path: PathBuf,
    /// The output file's bytes, when the linker produced one.
    pub produced: Option<Vec<u8>>,
}

impl LinkRun {
    /// True when the linker exited 0 *and* left an output file behind.
    pub fn succeeded(&self) -> bool {
        self.output.success() && self.produced.is_some()
    }

    /// The linker's own diagnostics, both streams, for a report block.
    pub fn diagnostics(&self) -> String {
        let mut s = String::new();
        if !self.output.stdout.trim().is_empty() {
            s.push_str(&format!("stdout:\n{}\n", self.output.stdout.trim_end()));
        }
        if !self.output.stderr.trim().is_empty() {
            s.push_str(&format!("stderr:\n{}\n", self.output.stderr.trim_end()));
        }
        if s.is_empty() {
            s.push_str("(the linker said nothing on either stream)");
        }
        s.trim_end().to_string()
    }

    /// A ready-made failure block set: the command line and the diagnostics.
    pub fn attach(&self, f: Failure) -> Failure {
        f.block("linker command", self.output.command_line())
            .block("linker output", self.diagnostics())
    }

    /// The failure a link that was supposed to work produces.
    pub fn failed(&self, doing: &str) -> Failure {
        let kind = if self.output.timed_out {
            FailureKind::Timeout
        } else if self.output.signal.is_some() {
            FailureKind::LinkerCrash
        } else {
            FailureKind::LinkFailed
        };
        let message = match (self.output.success(), &self.produced) {
            (true, None) => format!(
                "the linker exited 0 but wrote no output file ({})",
                self.out_path.display()
            ),
            _ => format!(
                "the linker did not link {doing}: {}",
                self.output.status_line()
            ),
        };
        self.attach(Failure::new(kind, message).note(format!("while {doing}")))
    }
}

/// A finished link whose output parsed as ELF.
#[derive(Debug, Clone)]
pub struct Linked {
    /// The link that produced it.
    pub run: LinkRun,
    /// The output file.
    pub path: PathBuf,
    /// Its bytes.
    pub bytes: Vec<u8>,
    /// Its parsed form.
    pub elf: Elf,
}

impl Linked {
    /// A failure with the output's structure already attached: the command line, the
    /// linker's diagnostics, and the program and section header tables.
    pub fn attach(&self, f: Failure) -> Failure {
        self.run
            .attach(f)
            .block("output program headers", self.elf.program_header_table())
            .block("output section headers", self.elf.section_header_table())
    }

    /// The address of a symbol in the output, as a failure-returning lookup.
    pub fn address_of(&self, name: &str) -> Result<u64, Failure> {
        self.elf.symbol_address(name).map_err(|e| {
            self.attach(
                Failure::new(FailureKind::Assertion, e.to_string()).note(format!(
                    "the test needs the address of '{name}' in the linked output; a linker that \
                     does not write a .symtab cannot be checked this way"
                )),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_puts_the_output_first_and_keeps_input_order() {
        let link = Link::new()
            .out("prog")
            .object("a.o", vec![1])
            .lib_dir("libs")
            .lib("x")
            .object("b.o", vec![2]);
        assert_eq!(
            link.argv(),
            vec!["-o", "prog", "a.o", "-L", "libs", "-l", "x", "b.o"]
        );
        assert_eq!(link.files.len(), 2);
    }

    #[test]
    fn a_written_file_is_not_an_input() {
        let link = Link::new().write_file("libs/libx.a", vec![1]).lib("x");
        assert_eq!(link.argv(), vec!["-o", "a.out", "-l", "x"]);
        assert_eq!(link.files[0].0, "libs/libx.a");
    }

    #[test]
    fn entry_flags_have_both_spellings() {
        assert_eq!(
            Link::new().entry("main").argv(),
            vec!["-o", "a.out", "-e", "main"]
        );
        assert_eq!(
            Link::new().entry_long("main").argv(),
            vec!["-o", "a.out", "--entry=main"]
        );
    }
}
