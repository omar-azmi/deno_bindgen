use std::borrow::Cow;
use std::io::Result;
use std::io::Write;
use std::path::Path;

use super::Generator;
use crate::inventory::Inventory;
use crate::inventory::Struct;
use crate::Type;

struct TypeScriptType<'a> {
  ty: Cow<'a, str>,
  is_custom_type: bool,
  is_optional: bool,
}

impl std::fmt::Display for TypeScriptType<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(&self.ty)?;
    Ok(())
  }
}

const option_type_typescript_suffix: &str = " | undefined";

impl TypeScriptType<'_> {
  fn into_raw<'a>(&self, ident: &'a str) -> Cow<'a, str> {
    if self.is_optional && self.is_custom_type {
      return Cow::Owned(format!("{ident} === undefined ? null : {ident}.ptr"));
    }
    if self.is_optional {
      return Cow::Owned(format!("{ident} === undefined ? null : {ident}"));
    }

    match self {
      _ if self.ty == "Uint8Array" && !self.is_custom_type => {
        Cow::Owned(format!("{ident},\n    {ident}.byteLength"))
      }
      Self {
        is_custom_type: true,
        ..
      } => Cow::Owned(format!("{ident}.ptr")),
      _ => Cow::Borrowed(ident),
    }
  }

  fn from_raw<'a>(&self, ident: &'a str) -> Option<String> {
    match self {
      Self {
        ty,
        is_custom_type: true,
        is_optional: true,
      } => {
        let custom_type =
          ty.strip_suffix(option_type_typescript_suffix).unwrap_or(ty);
        Some(format!(
          "{ident} === null ? undefined : {custom_type}.__constructor({ident})"
        ))
      }

      Self {
        ty,
        is_custom_type: true,
        is_optional: false,
      } => Some(format!("{ty}.__constructor({ident})")),

      Self {
        is_custom_type: false,
        is_optional: true,
        ..
      } => Some(format!("{ident} === null ? undefined : {ident}")),
      _ => None,
    }
  }

  fn apply_promise(&self, non_blocking: bool) -> Cow<'_, str> {
    if non_blocking {
      Cow::Owned(format!("Promise<{}>", self.ty))
    } else {
      Cow::Borrowed(self.ty.as_ref())
    }
  }
}

impl From<Type> for TypeScriptType<'_> {
  fn from(value: Type) -> Self {
    match value {
      Type::Optional(inner) => {
        let inner = Self::from(*inner);
        Self {
          ty: Cow::Owned(format!(
            "{}{option_type_typescript_suffix}",
            inner.ty,
          )),
          is_custom_type: inner.is_custom_type,
          is_optional: true,
        }
      }
      value => Self {
        ty: Cow::Borrowed(match value {
          Type::Void => "void",
          Type::Bool => "boolean",
          Type::Uint8
          | Type::Uint16
          | Type::Uint32
          | Type::Uint64
          | Type::Int8
          | Type::Int16
          | Type::Int32
          | Type::Int64
          | Type::Float32
          | Type::Float64 => "number",
          Type::Pointer => "Deno.PointerObject | null",
          Type::Buffer => "Uint8Array",
          Type::CustomType(name) => name,
          Type::Optional(_) => unreachable!(),
        }),
        is_custom_type: matches!(value, Type::CustomType(_)),
        is_optional: false,
      },
    }
  }
}

struct DenoFfiType(String);

impl std::fmt::Display for DenoFfiType {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(&self.0)?;
    Ok(())
  }
}

impl From<Type> for DenoFfiType {
  fn from(value: Type) -> Self {
    let ty = match value {
      Type::Void => "void",
      Type::Bool => "bool",
      Type::Uint8 => "u8",
      Type::Uint16 => "u16",
      Type::Uint32 => "u32",
      Type::Uint64 => "u64",
      Type::Int8 => "i8",
      Type::Int16 => "i16",
      Type::Int32 => "i32",
      Type::Int64 => "i64",
      Type::Float32 => "f32",
      Type::Float64 => "f64",
      Type::CustomType(..) | Type::Pointer => "pointer",
      Type::Buffer => "buffer",
      Type::Optional(..) => "pointer",
    };

    let mut raw = format!("'{}'", ty);

    // Complex types.
    if value == Type::Buffer {
      raw.push_str(",\n      'usize'");
    }

    Self(raw)
  }
}

pub struct Codegen<'a> {
  symbols: &'a [Inventory],
  target: &'a Path,
  lazy: bool,
}

impl<'a> Codegen<'a> {
  pub fn new(symbols: &'a [Inventory], target: &'a Path, lazy: bool) -> Self {
    Self {
      symbols,
      target,
      lazy,
    }
  }

  fn dlopen<W: Write>(&self, writer: &mut W) -> Result<()> {
    if self.lazy {
      return self.lazy_dlopen(writer);
    }
    writeln!(writer, "const {{ dlopen }} = Deno;\n")?;
    writeln!(writer, "const {{ symbols }} = dlopen({:?}, {{", self.target)?;
    self.write_symbols(writer)?;
    writeln!(writer, "}});\n")?;

    Ok(())
  }

  fn lazy_dlopen<W: Write>(&self, writer: &mut W) -> Result<()> {
    writeln!(writer, "let symbols: any;\n")?;
    writeln!(
      writer,
      "export function load(path: string = {:?}) {{",
      self.target
    )?;
    writeln!(writer, "  const {{ dlopen }} = Deno;\n")?;
    writeln!(writer, "  const {{ symbols: symbols_ }} = dlopen(path, {{")?;
    struct WrapperWriter<'a, W: Write> {
      writer: &'a mut W,
      indent: usize,
    }
    impl<W: Write> Write for WrapperWriter<'_, W> {
      fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Find newlines and indent them.
        for byte in buf {
          if *byte == b'\n' {
            self.writer.write_all(b"\n")?;
            self.writer.write_all(&vec![b' '; self.indent])?;
          } else {
            self.writer.write_all(&[*byte])?;
          }
        }

        Ok(buf.len())
      }

      fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
      }
    }
    write!(writer, "  ")?;
    let mut wr = WrapperWriter { writer, indent: 2 };
    self.write_symbols(&mut wr)?;
    writeln!(wr, "}});\n")?;
    write!(wr, "symbols = symbols_;")?;
    writeln!(writer, "\n}}\n")?;

    Ok(())
  }

  fn write_symbols<W: Write>(&self, writer: &mut W) -> Result<()> {
    fn format_bracket<W: Write, T>(
      writer: &mut W,
      items: &[T],
      callback: impl Fn(&mut W, &[T]) -> Result<()>,
    ) -> Result<()> {
      write!(writer, "[")?;
      if items.len() > 0 {
        write!(writer, "\n")?;
        callback(writer, items)?;
        writeln!(writer, "    ],")?;
      } else {
        writeln!(writer, "],")?;
      }

      Ok(())
    }

    for symbol in self.symbols {
      match symbol {
        Inventory::Symbol(symbol) => {
          writeln!(writer, "  {}: {{", symbol.name)?;
          write!(writer, "    parameters: ")?;
          format_bracket(writer, symbol.parameters, |writer, parameters| {
            for parameter in parameters {
              writeln!(writer, "      {},", DenoFfiType::from(*parameter))?;
            }
            Ok(())
          })?;
          writeln!(
            writer,
            "    result: {},",
            DenoFfiType::from(symbol.return_type)
          )?;
          writeln!(writer, "    nonblocking: {}", symbol.non_blocking)?;
          writeln!(writer, "  }},")?;
        }
        _ => {}
      }
    }

    Ok(())
  }

  fn exports<W: Write>(&self, writer: &mut W) -> Result<()> {
    fn format_paren<W: Write, T>(
      writer: &mut W,
      items: &[T],
      allow_empty: bool,
      callback: impl Fn(&mut W, &[T]) -> Result<()>,
      nesting_spaces: usize,
      delim: (char, &str),
    ) -> Result<()> {
      let (start, end) = delim;
      write!(writer, "{start}")?;
      if items.len() > 0 || allow_empty {
        write!(writer, "\n")?;
        callback(writer, items)?;
        write!(writer, "{:indent$}{end}", "", indent = nesting_spaces)?;
      } else {
        write!(writer, "{end}")?;
      }

      Ok(())
    }

    for symbol in self.symbols {
      match symbol {
        Inventory::Symbol(symbol) => {
          if !symbol.internal {
            write!(writer, "export ")?;
          }
          write!(writer, "function {}", symbol.name)?;
          format_paren(
            writer,
            symbol.parameters,
            false,
            |writer, parameters| {
              for (i, parameter) in parameters.iter().enumerate() {
                writeln!(
                  writer,
                  "  arg{}{}: {},",
                  i,
                  if matches!(parameter, Type::Optional(_)) {
                    "?"
                  } else {
                    ""
                  },
                  TypeScriptType::from(*parameter)
                )?;
              }
              Ok(())
            },
            0,
            ('(', ")"),
          )?;
          let ret_ty = TypeScriptType::from(symbol.return_type);
          writeln!(
            writer,
            ": {} {{",
            ret_ty.apply_promise(symbol.non_blocking)
          )?;
          let maybe_ret_transform = ret_ty.from_raw("ret");
          if maybe_ret_transform.is_some() {
            write!(writer, "  const ret = ")?;
          } else {
            write!(writer, "  return ")?;
          }
          write!(writer, "symbols.{}", symbol.name)?;
          format_paren(
            writer,
            symbol.parameters,
            false,
            |writer, parameters| {
              for (i, parameter) in parameters.iter().enumerate() {
                let ident = format!("arg{}", i);
                writeln!(
                  writer,
                  "    {},",
                  TypeScriptType::from(*parameter).into_raw(&ident)
                )?;
              }
              Ok(())
            },
            2,
            ('(', ")"),
          )?;

          if let Some(ret_transform) = maybe_ret_transform {
            write!(writer, "\n  return {ret_transform};")?;
          }
          writeln!(writer, "\n}}\n")?;
        }
        Inventory::Struct(Struct { name, methods }) => {
          write!(writer, "export class {name} ")?;

          format_paren(
            writer,
            &methods,
            false,
            |writer, methods| {
              writeln!(writer, "  ptr: Deno.PointerObject | null = null;\n")?;

              // Internal constructor.
              writeln!(
                writer,
                "  static __constructor(ptr: Deno.PointerObject | null) {{"
              )?;
              writeln!(
                writer,
                "    const self = Object.create({name}.prototype);"
              )?;
              writeln!(writer, "    self.ptr = ptr;")?;
              writeln!(writer, "    return self;")?;
              writeln!(writer, "  }}\n")?;

              // Dispose method (explicit resource management)
              writeln!(writer, "  [Symbol.dispose]() {{")?;
              writeln!(writer, "    this.dealloc();")?;
              writeln!(writer, "    this.ptr = null;")?;
              writeln!(writer, "  }}")?;

              for method in methods {
                let mut parameters = method.parameters;

                if !method.is_constructor && !method.is_static {
                  // Skip the self ptr argument.
                  parameters = &method.parameters[1..];
                }

                let method_name = if method.is_constructor {
                  "constructor"
                } else {
                  &method.name
                };

                write!(
                  writer,
                  "\n  {}{name}({parameters})",
                  if method.is_static { "static " } else { "" },
                  name = method_name,
                  parameters = parameters
                    .iter()
                    .enumerate()
                    .map(|(i, parameter)| {
                      format!(
                        "arg{}{}: {}",
                        i,
                        if matches!(parameter, Type::Optional(_)) {
                          "?"
                        } else {
                          ""
                        },
                        TypeScriptType::from(*parameter)
                      )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                )?;

                if !method.is_constructor {
                  let return_type = TypeScriptType::from(method.return_type);
                  writeln!(writer, ": {return_type} {{")?;
                } else {
                  // Typescript doesn't allow constructors to have a return type.
                  writeln!(writer, " {{")?;
                }

                // Apply name mangling.
                write!(writer, "    return __{}_{}", name, method.name)?;
                format_paren(
                  writer,
                  parameters,
                  !method.is_constructor && !method.is_static,
                  |writer, parameters| {
                    if !method.is_constructor && !method.is_static {
                      writeln!(writer, "      this.ptr,",)?;
                    }

                    for (i, parameter) in parameters.iter().enumerate() {
                      let ident = format!("arg{}", i);
                      writeln!(
                        writer,
                        "      {},",
                        TypeScriptType::from(*parameter).into_raw(&ident)
                      )?;
                    }

                    Ok(())
                  },
                  4,
                  ('(', ")"),
                )?;

                writeln!(writer, "\n  }}")?;
              }
              Ok(())
            },
            0,
            ('{', "}\n\n"),
          )?;
        }
      }
    }

    Ok(())
  }
}

impl<'a> Generator for Codegen<'a> {
  fn generate<W: Write>(&mut self, mut writer: W) -> Result<()> {
    fn prelude<W: Write>(writer: &mut W) -> Result<()> {
      writeln!(writer, "// deno-lint-ignore-file\n")?;
      writeln!(
        writer,
        "// This file is automatically generated by deno_bindgen."
      )?;
      writeln!(writer, "// Do not edit this file directly.\n")?;
      Ok(())
    }

    prelude(&mut writer)?;
    self.dlopen(&mut writer)?;
    self.exports(&mut writer)?;

    Ok(())
  }
}
