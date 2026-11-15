; Contextual captures precede the generic identifier rule for equal ranges.
(comment) @comment
(block . (identifier) @keyword)
(attribute (identifier) @property)
(object_elem key: (expression (variable_expr (identifier) @property)))
(get_attr (identifier) @property)
(function_call (identifier) @function)
(numeric_lit) @number
[(bool_lit) (null_lit)] @constant

[(string_lit) (quoted_template) (heredoc_template)] @string
; Expressions inside templates retain ordinary code colors.
(template_interpolation (expression) @embedded)
(template_if_intro (expression) @embedded)
(template_for_start (expression) @embedded)
(heredoc_identifier) @label
[(template_interpolation_start) (template_interpolation_end)
 (template_directive_start) (template_directive_end) (strip_marker)] @punctuation.special
["for" "in" "if" "else" "endif" "endfor"] @keyword
["=" "=>" "?" "+" "-" "*" "/" "%" "!" "==" "!=" ">" ">=" "<" "<=" "&&" "||"] @operator
[(block_start) (block_end) (object_start) (object_end)
 (tuple_start) (tuple_end) "(" ")" "[" "]"] @punctuation.bracket
["." "," ":" (ellipsis)] @punctuation.delimiter
(identifier) @variable
