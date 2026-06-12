use crate::JsRuleAction;
use crate::services::typed::Typed;
use biome_analyze::{
    FixKind, Rule, RuleDiagnostic, RuleDomain, RuleSource, context::RuleContext, declare_lint_rule,
};
use biome_console::markup;
use biome_js_factory::make;
use biome_js_syntax::{
    AnyJsArrowFunctionParameters, AnyJsCallArgument, AnyJsExpression, AnyJsFunctionBody,
    JsArrowFunctionExpression, JsAssignmentExpression, JsBinaryExpression, JsBinaryOperator,
    JsCallExpression, JsFunctionExpression, JsLogicalExpression, JsParameterList, JsReturnStatement,
    JsVariableDeclaration, T,
};
use biome_js_type_info::{ResolvedTypeData, Type, TypeData};
use biome_rowan::{AstNode, AstSeparatedList, BatchMutationExt, Text, TextRange, declare_node_union};
use biome_rule_options::use_includes::UseIncludesOptions;

declare_lint_rule! {
    /// Prefer `includes()` over `indexOf()`, `lastIndexOf()`, and simple `Array#some()` checks.
    ///
    /// `Array#indexOf()` and `Array#lastIndexOf()` return a numeric index and are commonly compared
    /// against `-1` to check for the presence of an element. `Array#some()` is sometimes used with a
    /// simple equality callback for the same purpose. `includes()` is more readable and expressive,
    /// and avoids off-by-one mistakes with the comparison operator.
    ///
    /// ## Examples
    ///
    /// ### Invalid
    ///
    /// ```ts,expect_diagnostic,file=invalid1.ts
    /// const arr = [1, 2, 3];
    /// arr.indexOf(1) !== -1;
    /// ```
    ///
    /// ```ts,expect_diagnostic,file=invalid2.ts
    /// const arr = [1, 2, 3];
    /// arr.lastIndexOf(1) !== -1;
    /// ```
    ///
    /// ```ts,expect_diagnostic,file=invalid3.ts
    /// const arr = [1, 2, 3];
    /// arr.some(x => x === 1);
    /// ```
    ///
    /// ```ts,expect_diagnostic,file=invalid4.ts
    /// const arr = [1, 2, 3];
    /// arr.indexOf(1) === -1;
    /// ```
    ///
    /// ### Valid
    ///
    /// ```ts
    /// const arr = [1, 2, 3];
    /// arr.includes(1);
    /// ```
    ///
    /// ```ts
    /// const arr = [1, 2, 3];
    /// // Positional use of indexOf is fine
    /// const pos = arr.indexOf(1);
    /// ```
    ///
    pub UseIncludes {
        version: "next",
        name: "useIncludes",
        language: "js",
        recommended: false,
        sources: &[RuleSource::EslintTypeScript("prefer-includes").inspired()],
        domains: &[RuleDomain::Types],
        fix_kind: FixKind::Unsafe,
    }
}

declare_node_union! {
    pub AnyUseIncludes = JsBinaryExpression | JsCallExpression
}

/// Whether the pattern represents a presence or absence check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckKind {
    /// `arr.indexOf(x) !== -1` → `arr.includes(x)`
    Includes,
    /// `arr.indexOf(x) === -1` → `!arr.includes(x)`
    NotIncludes,
}

pub enum UseIncludesPattern {
    IndexComparison {
        binary: JsBinaryExpression,
        call: JsCallExpression,
        kind: CheckKind,
        method_name: &'static str,
    },
    SomeCall {
        call: JsCallExpression,
        search_value: AnyJsExpression,
    },
}

impl UseIncludesPattern {
    fn range(&self) -> TextRange {
        match self {
            Self::IndexComparison { binary, .. } => binary.range(),
            Self::SomeCall { call, .. } => call.range(),
        }
    }
}

impl Rule for UseIncludes {
    type Query = Typed<AnyUseIncludes>;
    type State = UseIncludesPattern;
    type Signals = Option<Self::State>;
    type Options = UseIncludesOptions;

    fn run(ctx: &RuleContext<Self>) -> Self::Signals {
        match ctx.query() {
            AnyUseIncludes::JsBinaryExpression(binary) => detect_index_method_pattern(ctx, &binary),
            AnyUseIncludes::JsCallExpression(call) => detect_some_pattern(ctx, &call),
        }
    }

    fn diagnostic(_ctx: &RuleContext<Self>, state: &Self::State) -> Option<RuleDiagnostic> {
        match state {
            UseIncludesPattern::IndexComparison {
                kind,
                method_name,
                ..
            } => {
                let preferred = match kind {
                    CheckKind::Includes => "includes()",
                    CheckKind::NotIncludes => "!...includes()",
                };

                Some(
                    RuleDiagnostic::new(
                        rule_category!(),
                        state.range(),
                        markup! {
                            "Checking the result of "<Emphasis>{method_name}</Emphasis>" against "<Emphasis>"-1"</Emphasis>" to test for presence."
                        },
                    )
                    .note(markup! {
                        <Emphasis>{method_name}</Emphasis>" returns a numeric index, not a boolean. Comparing it against "<Emphasis>"-1"</Emphasis>" is error-prone and harder to read."
                    })
                    .note(markup! {
                        "Use "<Emphasis>{preferred}</Emphasis>" instead, which directly expresses the intent and returns a boolean."
                    }),
                )
            }
            UseIncludesPattern::SomeCall { .. } => Some(
                RuleDiagnostic::new(
                    rule_category!(),
                    state.range(),
                    markup! {
                        "Using "<Emphasis>"Array#some()"</Emphasis>" with a simple equality callback to test for presence."
                    },
                )
                .note(markup! {
                    "This callback only tests for equality against a single value, which can be expressed more clearly with "<Emphasis>".includes()"</Emphasis>"."
                }),
            ),
        }
    }

    fn action(ctx: &RuleContext<Self>, state: &Self::State) -> Option<JsRuleAction> {
        match state {
            UseIncludesPattern::IndexComparison {
                binary,
                call,
                kind,
                ..
            } => action_for_index_comparison(ctx, binary, call, *kind),
            UseIncludesPattern::SomeCall {
                call,
                search_value,
            } => action_for_some_call(ctx, call, search_value),
        }
    }
}

fn action_for_index_comparison(
    ctx: &RuleContext<UseIncludes>,
    binary: &JsBinaryExpression,
    call: &JsCallExpression,
    kind: CheckKind,
) -> Option<JsRuleAction> {
    let callee = call.callee().ok()?;
    let member = callee.as_js_static_member_expression()?;
    let object = member.object().ok()?;

    let args = call.arguments().ok()?;
    let search_arg = args.args().iter().next()?.ok()?;

    let includes_call = make_includes_call(object, search_arg);
    let replacement = match kind {
        CheckKind::Includes => AnyJsExpression::JsCallExpression(includes_call),
        CheckKind::NotIncludes => AnyJsExpression::JsUnaryExpression(make::js_unary_expression(
            make::token(T![!]),
            AnyJsExpression::JsCallExpression(includes_call),
        )),
    };

    let mut mutation = ctx.root().begin();
    mutation.replace_node(
        AnyJsExpression::JsBinaryExpression(binary.clone()),
        replacement,
    );

    Some(JsRuleAction::new(
        ctx.metadata().action_category(ctx.category(), ctx.group()),
        ctx.metadata().applicability(),
        match kind {
            CheckKind::Includes => {
                markup! { "Replace with "<Emphasis>".includes()"</Emphasis>"." }.to_owned()
            }
            CheckKind::NotIncludes => {
                markup! { "Replace with "<Emphasis>"!...includes()"</Emphasis>"." }.to_owned()
            }
        },
        mutation,
    ))
}

fn action_for_some_call(
    ctx: &RuleContext<UseIncludes>,
    call: &JsCallExpression,
    search_value: &AnyJsExpression,
) -> Option<JsRuleAction> {
    let callee = call.callee().ok()?;
    let member = callee.as_js_static_member_expression()?;
    let object = member.object().ok()?;

    let includes_call = make_includes_call(
        object,
        AnyJsCallArgument::AnyJsExpression(search_value.clone()),
    );

    let mut mutation = ctx.root().begin();
    mutation.replace_node(
        AnyJsExpression::JsCallExpression(call.clone()),
        AnyJsExpression::JsCallExpression(includes_call),
    );

    Some(JsRuleAction::new(
        ctx.metadata().action_category(ctx.category(), ctx.group()),
        ctx.metadata().applicability(),
        markup! { "Replace with "<Emphasis>".includes()"</Emphasis>"." }.to_owned(),
        mutation,
    ))
}

fn make_includes_call(object: AnyJsExpression, search_arg: AnyJsCallArgument) -> JsCallExpression {
    make::js_call_expression(
        make::js_static_member_expression(
            object,
            make::token(T![.]),
            make::js_name(make::ident("includes")).into(),
        )
        .into(),
        make::js_call_arguments(
            make::token(T!['(']),
            make::js_call_argument_list([search_arg], []),
            make::token(T![')']),
        ),
    )
    .build()
}

/// Attempts to detect the pattern `expr.indexOf(value) OP literal` or the
/// reversed form `literal OP expr.indexOf(value)` and normalise it so that
/// the left-hand side is always the `indexOf` call.
fn detect_index_method_pattern(
    ctx: &RuleContext<UseIncludes>,
    binary: &JsBinaryExpression,
) -> Option<UseIncludesPattern> {
    let operator = binary.operator().ok()?;
    let left = binary.left().ok()?;
    let right = binary.right().ok()?;

    if let Some((call, method_name)) = as_index_method_call(&left) {
        if !ensure_known_includes_type(ctx, &call) {
            return None;
        }
        return try_match_operator(binary, call, &right, operator, method_name);
    }

    if let Some((call, method_name)) = as_index_method_call(&right) {
        if !ensure_known_includes_type(ctx, &call) {
            return None;
        }
        let swapped = swap_operator(operator)?;
        return try_match_operator(binary, call, &left, swapped, method_name);
    }

    None
}

fn detect_some_pattern(
    ctx: &RuleContext<UseIncludes>,
    call: &JsCallExpression,
) -> Option<UseIncludesPattern> {
    let callee = call.callee().ok()?;
    let member = callee.as_js_static_member_expression()?;
    let member_name = member.member().ok()?;
    let name = member_name.as_js_name()?;
    if name.value_token().ok()?.text_trimmed() != "some" {
        return None;
    }

    let object = member.object().ok()?;
    if !ensure_known_array_type(ctx, &object) {
        return None;
    }

    let args = call.arguments().ok()?;
    if args.args().len() != 1 {
        return None;
    }

    let callback = args.args().first()?.ok()?;
    let search_value = match callback {
        AnyJsCallArgument::AnyJsExpression(AnyJsExpression::JsArrowFunctionExpression(function)) => {
            callback_arrow_function_match(&function)?
        }
        AnyJsCallArgument::AnyJsExpression(AnyJsExpression::JsFunctionExpression(function)) => {
            callback_function_match(&function)?
        }
        _ => return None,
    };

    Some(UseIncludesPattern::SomeCall {
        call: call.clone(),
        search_value,
    })
}

/// Returns `Some((call, method_name))` when `expr` is a call to `something.indexOf(...)` or
/// `something.lastIndexOf(...)`.
fn as_index_method_call(expr: &AnyJsExpression) -> Option<(JsCallExpression, &'static str)> {
    let binding = expr.clone().omit_parentheses();
    let call = binding.as_js_call_expression()?.clone();
    let callee = call.callee().ok()?;
    let member = callee.as_js_static_member_expression()?;
    let name = member.member().ok()?;
    let js_name = name.as_js_name()?;
    let method_name = match js_name.value_token().ok()?.text_trimmed() {
        "indexOf" => "indexOf()",
        "lastIndexOf" => "lastIndexOf()",
        _ => return None,
    };
    // Must have exactly one argument (the search value). If a `fromIndex` is
    // supplied we leave it alone because `includes(value, fromIndex)` has
    // different semantics from `indexOf(value, fromIndex) !== -1` when
    // `fromIndex` is negative.
    let args = call.arguments().ok()?;
    if args.args().len() != 1 {
        return None;
    }
    Some((call, method_name))
}

/// Given the index method call on the left and the literal on the right,
/// decide whether the comparison is a presence check, an absence check, or
/// something we should not touch.
fn try_match_operator(
    binary: &JsBinaryExpression,
    call: JsCallExpression,
    other: &AnyJsExpression,
    operator: JsBinaryOperator,
    method_name: &'static str,
) -> Option<UseIncludesPattern> {
    let kind = match operator {
        JsBinaryOperator::StrictInequality | JsBinaryOperator::Inequality
            if is_negative_one(other) =>
        {
            CheckKind::Includes
        }
        JsBinaryOperator::GreaterThanOrEqual if is_zero(other) => CheckKind::Includes,
        JsBinaryOperator::GreaterThan if is_negative_one(other) => CheckKind::Includes,

        JsBinaryOperator::StrictEquality
        | JsBinaryOperator::Equality
        | JsBinaryOperator::LessThanOrEqual
            if is_negative_one(other) =>
        {
            CheckKind::NotIncludes
        }
        JsBinaryOperator::LessThan if is_zero(other) => CheckKind::NotIncludes,

        _ => return None,
    };

    Some(UseIncludesPattern::IndexComparison {
        binary: binary.clone(),
        call,
        kind,
        method_name,
    })
}

fn swap_operator(op: JsBinaryOperator) -> Option<JsBinaryOperator> {
    Some(match op {
        JsBinaryOperator::GreaterThan => JsBinaryOperator::LessThan,
        JsBinaryOperator::GreaterThanOrEqual => JsBinaryOperator::LessThanOrEqual,
        JsBinaryOperator::LessThan => JsBinaryOperator::GreaterThan,
        JsBinaryOperator::LessThanOrEqual => JsBinaryOperator::GreaterThanOrEqual,
        JsBinaryOperator::StrictEquality => JsBinaryOperator::StrictEquality,
        JsBinaryOperator::Equality => JsBinaryOperator::Equality,
        JsBinaryOperator::StrictInequality => JsBinaryOperator::StrictInequality,
        JsBinaryOperator::Inequality => JsBinaryOperator::Inequality,
        _ => return None,
    })
}

fn is_negative_one(expr: &AnyJsExpression) -> bool {
    let expr = expr.clone().omit_parentheses();

    if let Some(n) = as_number_literal(&expr) {
        return n == -1.0;
    }

    let Some(unary) = expr.as_js_unary_expression() else {
        return false;
    };
    let is_minus = unary
        .operator_token()
        .ok()
        .is_some_and(|t| t.kind() == biome_js_syntax::JsSyntaxKind::MINUS);
    if !is_minus {
        return false;
    }
    unary
        .argument()
        .ok()
        .and_then(|arg| as_number_literal(&arg))
        .is_some_and(|n| n == 1.0)
}

fn is_zero(expr: &AnyJsExpression) -> bool {
    as_number_literal(&expr.clone().omit_parentheses()).is_some_and(|n| n == 0.0)
}

fn as_number_literal(expr: &AnyJsExpression) -> Option<f64> {
    expr.clone()
        .omit_parentheses()
        .as_any_js_literal_expression()
        .and_then(|lit| lit.as_js_number_literal_expression().cloned())
        .and_then(|n| n.as_number())
}

fn ensure_known_includes_type(ctx: &RuleContext<UseIncludes>, call: &JsCallExpression) -> bool {
    let callee = call.callee().ok();
    let member = callee
        .as_ref()
        .and_then(|c| c.as_js_static_member_expression());
    let object = member.and_then(|m| m.object().ok());

    let Some(object) = object else {
        return false;
    };

    all_type_variants_match(&ctx.type_of_expression(&object), |current, raw| {
        current.is_string_or_string_literal()
            || current.is_array_of(|_| true)
            || matches!(raw, TypeData::Tuple(_))
    })
}

fn ensure_known_array_type(ctx: &RuleContext<UseIncludes>, object: &AnyJsExpression) -> bool {
    all_type_variants_match(&ctx.type_of_expression(object), |current, raw| {
        current.is_array_of(|_| true) || matches!(raw, TypeData::Tuple(_))
    })
}

fn all_type_variants_match(ty: &Type, mut predicate: impl FnMut(&Type, &TypeData) -> bool) -> bool {
    let mut saw_variant = false;
    let mut pending = vec![ty.clone()];

    while let Some(current) = pending.pop() {
        if current.is_union() {
            let mut variants = current.flattened_union_variants().peekable();
            if variants.peek().is_none() {
                return false;
            }
            saw_variant = true;
            pending.extend(variants);
            continue;
        }

        let Some(raw) = current.resolved_data().map(ResolvedTypeData::as_raw_data) else {
            return false;
        };

        match raw {
            TypeData::Generic(generic) if generic.constraint.is_known() => {
                let Some(constraint) = current.resolve(&generic.constraint) else {
                    return false;
                };
                pending.push(constraint);
            }
            TypeData::Generic(_) => return false,
            _ if predicate(&current, raw) => saw_variant = true,
            _ => return false,
        }
    }

    saw_variant
}

fn extract_simple_compare_match(
    expression: &JsBinaryExpression,
    parameter_name: &Text,
) -> Option<AnyJsExpression> {
    if expression.operator().ok()? != JsBinaryOperator::StrictEquality {
        return None;
    }

    let (left, right) = (expression.left().ok()?, expression.right().ok()?);

    if left.to_trimmed_text() == *parameter_name {
        Some(right)
    } else if right.to_trimmed_text() == *parameter_name {
        Some(left)
    } else {
        None
    }
}

fn find_some_comparable_expression(
    body: &AnyJsFunctionBody,
    parameter_name: &Text,
    return_statement_required: bool,
) -> Option<AnyJsExpression> {
    let has_invalid_expression = body.syntax().descendants().find(|node| {
        JsAssignmentExpression::can_cast(node.kind())
            || JsVariableDeclaration::can_cast(node.kind())
            || JsLogicalExpression::can_cast(node.kind())
    });

    if has_invalid_expression.is_some() {
        return None;
    }

    let mut binary_expressions = body
        .syntax()
        .descendants()
        .filter_map(JsBinaryExpression::cast);

    let binary_expression = binary_expressions.next()?;
    if binary_expressions.next().is_some() {
        return None;
    }

    let mut return_statements = body
        .syntax()
        .descendants()
        .filter_map(JsReturnStatement::cast);
    let has_one_or_more_return_statements = return_statements.next().is_some();
    let has_two_or_more_return_statements = return_statements.next().is_some();

    if has_two_or_more_return_statements {
        return None;
    }

    if return_statement_required && !has_one_or_more_return_statements {
        return None;
    }

    extract_simple_compare_match(&binary_expression, parameter_name)
}

fn extract_function_parameter_name(parameters: &JsParameterList) -> Option<Text> {
    if parameters.len() != 1 {
        return None;
    }

    Some(parameters.first()?.ok()?.to_trimmed_text())
}

fn extract_parameter_name(parameters: &AnyJsArrowFunctionParameters) -> Option<Text> {
    if parameters.len() != 1 {
        return None;
    }

    match parameters {
        AnyJsArrowFunctionParameters::AnyJsBinding(binding) => Some(binding.to_trimmed_text()),
        AnyJsArrowFunctionParameters::JsParameters(param) => param
            .items()
            .first()?
            .ok()
            .map(|item| item.to_trimmed_text()),
    }
}

fn callback_function_match(function: &JsFunctionExpression) -> Option<AnyJsExpression> {
    if function.async_token().is_some() || function.star_token().is_some() {
        return None;
    }

    let function_parameters = function.parameters().ok()?.items();
    let parameter_name = extract_function_parameter_name(&function_parameters)?;
    let binding = function.body().ok()?;
    let body = binding
        .syntax()
        .descendants()
        .find_map(AnyJsFunctionBody::cast)?;

    find_some_comparable_expression(&body, &parameter_name, true)
}

fn callback_arrow_function_match(function: &JsArrowFunctionExpression) -> Option<AnyJsExpression> {
    if function.async_token().is_some() {
        return None;
    }

    let parameters = function.parameters().ok()?;
    let parameter_name = extract_parameter_name(&parameters)?;
    let body = function.body().ok()?;

    find_some_comparable_expression(&body, &parameter_name, false)
}
