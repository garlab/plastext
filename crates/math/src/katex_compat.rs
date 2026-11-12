//! Compatibility with katex-rs 0.3.0's HTML builders.

use std::sync::{Arc, OnceLock};

use katex_native::katex::{
    KatexContext,
    define_function::{FunctionContext, FunctionHandler},
    parser::parse_node::{ParseNode, ParseNodeOrdGroup},
    types::ParseError,
};

static SQRT_HANDLER: OnceLock<FunctionHandler> = OnceLock::new();

pub(super) fn context() -> KatexContext {
    let mut context = KatexContext::default();
    let sqrt = Arc::make_mut(context.functions.get_mut("\\sqrt").expect("sqrt function"));
    SQRT_HANDLER.get_or_init(|| sqrt.handler.expect("sqrt parser"));
    sqrt.handler = Some(group_sqrt_argument);
    context
}

fn group_sqrt_argument(
    context: FunctionContext<'_, '_, '_>,
    mut args: Vec<ParseNode>,
    optional_args: Vec<Option<ParseNode>>,
) -> Result<ParseNode, ParseError> {
    // katex-rs's sqrt HTML builder adds the radical's left padding only to a
    // DomSpan. An unbraced symbol produces a SymbolNode instead, so `\sqrt3`
    // overlaps the radical. Group the already-parsed argument as in `\sqrt{3}`:
    // this also handles macro expansion and optional root indices without
    // rewriting source or adding another TeX tokenizer. Remove this adapter
    // once the upstream sqrt builder wraps non-span radicands before padding.
    if !matches!(args[0], ParseNode::OrdGroup(_)) {
        let body = args.remove(0);
        args.push(ParseNode::OrdGroup(ParseNodeOrdGroup {
            mode: body.mode(),
            loc: None,
            body: vec![body],
            semisimple: None,
        }));
    }
    SQRT_HANDLER.get().expect("sqrt parser initialized")(context, args, optional_args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_radicals_match_upstream_grouped_layouts() {
        let fixed = context();
        let upstream = KatexContext::default();
        for display in [false, true] {
            for (bare, grouped) in [
                (r"\sqrt3", r"\sqrt{3}"),
                (r"\sqrt n", r"\sqrt{n}"),
                (r"\sqrt\alpha", r"\sqrt{\alpha}"),
                (r"\sqrt[3]3", r"\sqrt[3]{3}"),
                (r"\frac1{\sqrt3}", r"\frac1{\sqrt{3}}"),
                (r"x^{\sqrt3}", r"x^{\sqrt{3}}"),
                (r"\sqrt{\sqrt3}", r"\sqrt{\sqrt{3}}"),
                (r"\sqrt{\frac1{\sqrt3}}", r"\sqrt{\frac1{\sqrt{3}}}"),
                (r"\sqrt{A_0}", r"\sqrt{A_0}"),
                (r"\sqrt{(1+3a)(3+a)}", r"\sqrt{(1+3a)(3+a)}"),
            ] {
                assert_eq!(
                    katex_native::layout(&fixed, bare, display).unwrap(),
                    katex_native::layout(&upstream, grouped, display).unwrap(),
                    "{bare}, display={display}"
                );
            }
        }
    }

    #[test]
    fn user_defined_sqrt_still_overrides_the_builtin() {
        let context = context();
        assert_eq!(
            katex_native::layout(&context, r"\def\sqrt#1{#1+1}\sqrt3", false).unwrap(),
            katex_native::layout(&context, "3+1", false).unwrap()
        );
    }
}
