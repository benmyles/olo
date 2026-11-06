; Kotlin (tree-sitter-kotlin-ng ships no queries): Olo's own, for the
; classes Olo colours (comments, strings, numbers, keywords, types,
; functions, constants, annotations).

(line_comment) @comment
(block_comment) @comment
(shebang) @comment

(string_literal) @string
(multiline_string_literal) @string
(character_literal) @string
(escape_sequence) @string.escape
(interpolation) @embedded

(number_literal) @number
(float_literal) @number

((identifier) @constant.builtin
  (#any-of? @constant.builtin "true" "false" "null"))

(annotation) @attribute
(file_annotation) @attribute
(label) @label

(function_declaration name: (identifier) @function)
(call_expression (identifier) @function.call)
(call_expression (navigation_expression (identifier) @function.call .))

(class_declaration name: (identifier) @type)
(object_declaration name: (identifier) @type)
(type_alias type: (identifier) @type)
(user_type (identifier) @type)

(this_expression) @variable.builtin
(super_expression) @variable.builtin

(import (qualified_identifier) @module)
(package_header (qualified_identifier) @module)

[
  "abstract" "actual" "annotation" "as" "by" "catch" "class" "companion"
  "const" "constructor" "crossinline" "data" "delegate" "do" "dynamic"
  "else" "enum" "expect" "external" "field" "file" "final" "finally" "for"
  "fun" "get" "if" "import" "in" "infix" "init" "inline" "inner"
  "interface" "internal" "is" "lateinit" "noinline" "object" "open"
  "operator" "out" "override" "package" "param" "private" "property"
  "protected" "public" "receiver" "return" "sealed" "set" "setparam"
  "suspend" "tailrec" "throw" "try" "typealias" "val" "value" "var"
  "vararg" "when" "where" "while"
] @keyword
