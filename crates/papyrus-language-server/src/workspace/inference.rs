use crate::semantic::{
    Declaration, DeclarationKind, SemanticExpression, SemanticLiteralKind, TypeRef,
};

use super::{
    IndexedDocument, WorkspaceIndex,
    type_system::{self, ValueType},
};

const MAX_EXPRESSION_DEPTH: usize = 64;

pub(super) trait ExpressionContext {
    fn resolve_visible_name<'a>(
        &'a self,
        current: &'a IndexedDocument,
        name: &str,
        offset: usize,
    ) -> Option<&'a Declaration>;

    fn unique_script<'a>(&'a self, name: &str) -> Option<(&'a IndexedDocument, &'a Declaration)>;

    fn members_of<'a>(&'a self, script: &'a Declaration) -> Vec<&'a Declaration>;

    fn members_of_type<'a>(
        &'a self,
        current: &'a IndexedDocument,
        type_name: &str,
    ) -> Vec<&'a Declaration>;

    fn resolve_visible_name_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        name: &str,
        offset: usize,
    ) -> Resolution<&'a Declaration> {
        self.resolve_visible_name(current, name, offset)
            .map_or(Resolution::Unsupported, Resolution::Resolved)
    }

    fn resolve_script_outcome<'a>(
        &'a self,
        name: &str,
    ) -> Resolution<(&'a IndexedDocument, &'a Declaration)> {
        self.unique_script(name)
            .map_or(Resolution::Unsupported, Resolution::Resolved)
    }

    fn members_of_type_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        type_name: &str,
    ) -> Resolution<Vec<&'a Declaration>> {
        let members = self.members_of_type(current, type_name);
        if members.is_empty() {
            Resolution::Unsupported
        } else {
            Resolution::Resolved(members)
        }
    }

    fn declaration_name<'a>(&'a self, declaration: &'a Declaration) -> &'a str {
        &declaration.name
    }

    fn name_matches(&self, declaration: &Declaration, name: &str) -> bool {
        self.declaration_name(declaration)
            .eq_ignore_ascii_case(name)
    }
}

impl ExpressionContext for WorkspaceIndex {
    fn resolve_visible_name<'a>(
        &'a self,
        current: &'a IndexedDocument,
        name: &str,
        offset: usize,
    ) -> Option<&'a Declaration> {
        WorkspaceIndex::resolve_visible_name(self, current, name, offset)
    }

    fn unique_script<'a>(&'a self, name: &str) -> Option<(&'a IndexedDocument, &'a Declaration)> {
        WorkspaceIndex::unique_script(self, name)
    }

    fn members_of<'a>(&'a self, script: &'a Declaration) -> Vec<&'a Declaration> {
        WorkspaceIndex::members_of(self, script)
    }

    fn members_of_type<'a>(
        &'a self,
        current: &'a IndexedDocument,
        type_name: &str,
    ) -> Vec<&'a Declaration> {
        WorkspaceIndex::members_of_type(self, current, type_name)
    }

    fn resolve_visible_name_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        name: &str,
        offset: usize,
    ) -> Resolution<&'a Declaration> {
        WorkspaceIndex::resolve_visible_name_outcome(self, current, name, offset)
    }

    fn resolve_script_outcome<'a>(
        &'a self,
        name: &str,
    ) -> Resolution<(&'a IndexedDocument, &'a Declaration)> {
        WorkspaceIndex::unique_script_outcome(self, name)
    }

    fn members_of_type_outcome<'a>(
        &'a self,
        current: &'a IndexedDocument,
        type_name: &str,
    ) -> Resolution<Vec<&'a Declaration>> {
        WorkspaceIndex::members_of_type_outcome(self, current, type_name)
    }
}

pub(super) enum Resolution<T> {
    Resolved(T),
    Missing,
    Ambiguous,
    Unsupported,
}

impl<T> Resolution<T> {
    pub(super) fn map<U>(self, map: impl FnOnce(T) -> U) -> Resolution<U> {
        match self {
            Self::Resolved(value) => Resolution::Resolved(map(value)),
            Self::Missing => Resolution::Missing,
            Self::Ambiguous => Resolution::Ambiguous,
            Self::Unsupported => Resolution::Unsupported,
        }
    }

    pub(super) fn into_option(self) -> Option<T> {
        match self {
            Self::Resolved(value) => Some(value),
            Self::Missing | Self::Ambiguous | Self::Unsupported => None,
        }
    }
}

enum ExpressionResolution<'a> {
    Declaration(&'a Declaration),
    Script(&'a IndexedDocument, &'a Declaration),
    Value(ValueType),
}

pub(super) fn resolve_member_expression<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    receiver: &SemanticExpression,
    member: &str,
) -> Option<&'a Declaration> {
    resolve_member_expression_outcome(context, current, receiver, member).into_option()
}

pub(super) fn resolve_member_expression_outcome<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    receiver: &SemanticExpression,
    member: &str,
) -> Resolution<&'a Declaration> {
    match resolve_expression(context, current, receiver, 0) {
        Resolution::Resolved(receiver) => {
            resolve_member_from_resolution(context, current, receiver, member)
        }
        Resolution::Ambiguous => Resolution::Ambiguous,
        Resolution::Missing | Resolution::Unsupported => Resolution::Unsupported,
    }
}

pub(super) fn members_for_expression<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    expression: &SemanticExpression,
) -> Vec<&'a Declaration> {
    match resolve_expression(context, current, expression, 0) {
        Resolution::Resolved(ExpressionResolution::Declaration(declaration)) => declaration
            .ty
            .as_ref()
            .and_then(TypeRef::scalar_name)
            .map(|ty| context.members_of_type(current, ty))
            .unwrap_or_default(),
        Resolution::Resolved(ExpressionResolution::Script(_, script)) => context.members_of(script),
        Resolution::Resolved(ExpressionResolution::Value(ValueType::Known(ty))) => ty
            .scalar_name()
            .map(|ty| context.members_of_type(current, ty))
            .unwrap_or_default(),
        Resolution::Resolved(ExpressionResolution::Value(ValueType::None | ValueType::Void)) => {
            Vec::new()
        }
        Resolution::Missing | Resolution::Ambiguous | Resolution::Unsupported => Vec::new(),
    }
}

fn resolve_expression<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    expression: &SemanticExpression,
    depth: usize,
) -> Resolution<ExpressionResolution<'a>> {
    if depth >= MAX_EXPRESSION_DEPTH {
        return Resolution::Unsupported;
    }
    match expression {
        SemanticExpression::Identifier {
            name, byte_offset, ..
        } => {
            if name.eq_ignore_ascii_case("self") {
                let Some(script_name) = current.semantic.script_name.as_deref() else {
                    return Resolution::Unsupported;
                };
                let effective_name = context
                    .unique_script(script_name)
                    .map(|(_, script)| context.declaration_name(script))
                    .unwrap_or(script_name);
                return Resolution::Resolved(ExpressionResolution::Value(ValueType::Known(
                    TypeRef {
                        name: effective_name.to_owned(),
                        array: false,
                    },
                )));
            }
            if name.eq_ignore_ascii_case("parent") {
                let Some(parent_name) = current.semantic.parent_script.as_deref() else {
                    return Resolution::Unsupported;
                };
                return match context.resolve_script_outcome(parent_name) {
                    Resolution::Resolved((_, script)) => Resolution::Resolved(
                        ExpressionResolution::Value(ValueType::Known(TypeRef {
                            name: context.declaration_name(script).to_owned(),
                            array: false,
                        })),
                    ),
                    Resolution::Missing | Resolution::Unsupported => Resolution::Unsupported,
                    Resolution::Ambiguous => Resolution::Ambiguous,
                };
            }
            match context.resolve_visible_name_outcome(current, name, *byte_offset) {
                Resolution::Resolved(declaration) => {
                    if declaration.kind == DeclarationKind::Script {
                        return match context.resolve_script_outcome(&declaration.name) {
                            Resolution::Resolved((document, script)) => {
                                Resolution::Resolved(ExpressionResolution::Script(document, script))
                            }
                            Resolution::Missing => Resolution::Missing,
                            Resolution::Ambiguous => Resolution::Ambiguous,
                            Resolution::Unsupported => Resolution::Unsupported,
                        };
                    }
                    Resolution::Resolved(ExpressionResolution::Declaration(declaration))
                }
                Resolution::Ambiguous => Resolution::Ambiguous,
                Resolution::Missing => script_resolution(context, name),
                Resolution::Unsupported => match script_resolution(context, name) {
                    Resolution::Resolved(value) => Resolution::Resolved(value),
                    Resolution::Ambiguous => Resolution::Ambiguous,
                    Resolution::Missing | Resolution::Unsupported => Resolution::Unsupported,
                },
            }
        }
        SemanticExpression::Member { object, member, .. } => {
            match resolve_expression(context, current, object, depth + 1) {
                Resolution::Resolved(receiver) => {
                    match resolve_member_from_resolution(context, current, receiver, member) {
                        Resolution::Resolved(declaration) => {
                            Resolution::Resolved(ExpressionResolution::Declaration(declaration))
                        }
                        Resolution::Missing => Resolution::Missing,
                        Resolution::Ambiguous => Resolution::Ambiguous,
                        Resolution::Unsupported => Resolution::Unsupported,
                    }
                }
                Resolution::Ambiguous => Resolution::Ambiguous,
                Resolution::Missing | Resolution::Unsupported => Resolution::Unsupported,
            }
        }
        SemanticExpression::Call { function, .. } => {
            let function = match resolve_expression(context, current, function, depth + 1) {
                Resolution::Resolved(ExpressionResolution::Declaration(function)) => function,
                Resolution::Resolved(
                    ExpressionResolution::Script(_, _) | ExpressionResolution::Value(_),
                )
                | Resolution::Unsupported => return Resolution::Unsupported,
                Resolution::Missing => return Resolution::Missing,
                Resolution::Ambiguous => return Resolution::Ambiguous,
            };
            if !matches!(
                function.kind,
                DeclarationKind::Function | DeclarationKind::Event
            ) {
                return Resolution::Unsupported;
            }
            Resolution::Resolved(ExpressionResolution::Value(
                function
                    .ty
                    .clone()
                    .map_or(ValueType::Void, ValueType::Known),
            ))
        }
        SemanticExpression::Subscript { array, .. } => {
            let mut ty = match expression_type_outcome(context, current, array, depth + 1) {
                Resolution::Resolved(ValueType::Known(ty)) => ty,
                Resolution::Resolved(ValueType::None | ValueType::Void) => {
                    return Resolution::Unsupported;
                }
                Resolution::Missing => return Resolution::Missing,
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::Unsupported => return Resolution::Unsupported,
            };
            if !ty.array {
                return Resolution::Unsupported;
            }
            ty.array = false;
            Resolution::Resolved(ExpressionResolution::Value(ValueType::Known(ty)))
        }
        SemanticExpression::Cast { ty, .. } | SemanticExpression::New { ty, .. } => {
            Resolution::Resolved(ExpressionResolution::Value(ValueType::Known(ty.clone())))
        }
        SemanticExpression::TypeTest { .. } => {
            Resolution::Resolved(ExpressionResolution::Value(type_system::known("Bool")))
        }
        SemanticExpression::Parenthesized { value, .. } => {
            resolve_expression(context, current, value, depth + 1)
        }
        SemanticExpression::Literal { kind, .. } => {
            let value = match kind {
                SemanticLiteralKind::Bool => type_system::known("Bool"),
                SemanticLiteralKind::Int => type_system::known("Int"),
                SemanticLiteralKind::Float => type_system::known("Float"),
                SemanticLiteralKind::String => type_system::known("String"),
                SemanticLiteralKind::None => ValueType::None,
            };
            Resolution::Resolved(ExpressionResolution::Value(value))
        }
        SemanticExpression::Unary {
            operator, argument, ..
        } => match expression_type_outcome(context, current, argument, depth + 1) {
            Resolution::Resolved(argument) => type_system::unary_result(*operator, &argument)
                .map_or(Resolution::Unsupported, |value| {
                    Resolution::Resolved(ExpressionResolution::Value(value))
                }),
            Resolution::Missing => Resolution::Missing,
            Resolution::Ambiguous => Resolution::Ambiguous,
            Resolution::Unsupported => Resolution::Unsupported,
        },
        SemanticExpression::Binary {
            left,
            operator,
            right,
            ..
        } => {
            let left = match expression_type_outcome(context, current, left, depth + 1) {
                Resolution::Resolved(left) => left,
                Resolution::Missing => return Resolution::Missing,
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::Unsupported => return Resolution::Unsupported,
            };
            let right = match expression_type_outcome(context, current, right, depth + 1) {
                Resolution::Resolved(right) => right,
                Resolution::Missing => return Resolution::Missing,
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::Unsupported => return Resolution::Unsupported,
            };
            type_system::binary_result(*operator, &left, &right)
                .map_or(Resolution::Unsupported, |value| {
                    Resolution::Resolved(ExpressionResolution::Value(value))
                })
        }
    }
}

pub(super) fn resolve_expression_declaration_outcome<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    expression: &SemanticExpression,
) -> Resolution<&'a Declaration> {
    match resolve_expression(context, current, expression, 0) {
        Resolution::Resolved(ExpressionResolution::Declaration(declaration)) => {
            Resolution::Resolved(declaration)
        }
        Resolution::Resolved(ExpressionResolution::Script(_, _)) => Resolution::Unsupported,
        Resolution::Resolved(ExpressionResolution::Value(_)) => Resolution::Unsupported,
        Resolution::Missing => Resolution::Missing,
        Resolution::Ambiguous => Resolution::Ambiguous,
        Resolution::Unsupported => Resolution::Unsupported,
    }
}

pub(super) fn expression_type_outcome<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    expression: &SemanticExpression,
    depth: usize,
) -> Resolution<ValueType> {
    match resolve_expression(context, current, expression, depth) {
        Resolution::Resolved(ExpressionResolution::Declaration(declaration)) => {
            if matches!(
                declaration.kind,
                DeclarationKind::Function | DeclarationKind::Event
            ) {
                Resolution::Resolved(ValueType::Void)
            } else {
                declaration
                    .ty
                    .clone()
                    .map(ValueType::Known)
                    .map_or(Resolution::Unsupported, Resolution::Resolved)
            }
        }
        Resolution::Resolved(ExpressionResolution::Script(_, script)) => {
            Resolution::Resolved(ValueType::Known(TypeRef {
                name: context.declaration_name(script).to_owned(),
                array: false,
            }))
        }
        Resolution::Resolved(ExpressionResolution::Value(ty)) => Resolution::Resolved(ty),
        Resolution::Missing => Resolution::Missing,
        Resolution::Ambiguous => Resolution::Ambiguous,
        Resolution::Unsupported => Resolution::Unsupported,
    }
}

fn resolve_member_from_resolution<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    receiver: ExpressionResolution<'a>,
    member: &str,
) -> Resolution<&'a Declaration> {
    match receiver {
        ExpressionResolution::Declaration(declaration) => {
            let Some(ty) = declaration.ty.as_ref().and_then(TypeRef::scalar_name) else {
                return Resolution::Unsupported;
            };
            resolve_named_type_member(context, current, ty, member)
        }
        ExpressionResolution::Value(ValueType::Known(ty)) => {
            let Some(ty) = ty.scalar_name() else {
                return Resolution::Unsupported;
            };
            resolve_named_type_member(context, current, ty, member)
        }
        ExpressionResolution::Value(ValueType::None | ValueType::Void) => Resolution::Unsupported,
        ExpressionResolution::Script(document, script) => unique_named_outcome(
            document
                .semantic
                .declarations
                .iter()
                .filter(|declaration| {
                    declaration.kind == DeclarationKind::Function
                        && declaration.is_global
                        && declaration.container.is_none()
                        && declaration
                            .owner_script
                            .as_deref()
                            .is_some_and(|owner| owner.eq_ignore_ascii_case(&script.name))
                })
                .collect(),
            member,
            context,
        ),
    }
}

fn resolve_named_type_member<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    type_name: &str,
    member: &str,
) -> Resolution<&'a Declaration> {
    match context.members_of_type_outcome(current, type_name) {
        Resolution::Resolved(declarations) => unique_named_outcome(declarations, member, context),
        Resolution::Ambiguous => Resolution::Ambiguous,
        Resolution::Missing | Resolution::Unsupported => Resolution::Unsupported,
    }
}

fn script_resolution<'a>(
    context: &'a impl ExpressionContext,
    name: &str,
) -> Resolution<ExpressionResolution<'a>> {
    match context.resolve_script_outcome(name) {
        Resolution::Resolved((document, script)) => {
            Resolution::Resolved(ExpressionResolution::Script(document, script))
        }
        Resolution::Missing => Resolution::Missing,
        Resolution::Ambiguous => Resolution::Ambiguous,
        Resolution::Unsupported => Resolution::Unsupported,
    }
}

fn unique_named_outcome<'a>(
    declarations: Vec<&'a Declaration>,
    name: &str,
    context: &'a impl ExpressionContext,
) -> Resolution<&'a Declaration> {
    let mut matches = declarations
        .into_iter()
        .filter(|declaration| context.name_matches(declaration, name));
    let Some(first) = matches.next() else {
        return Resolution::Missing;
    };
    if matches.next().is_some() {
        Resolution::Ambiguous
    } else {
        Resolution::Resolved(first)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use lsp_types::Position;

    use crate::config::WorkspaceConfig;

    use super::super::{WorkspaceIndex, path_to_file_uri};

    #[test]
    fn infers_chained_cast_parenthesized_array_and_self_receivers() {
        let root = temp_root("expression-inference");
        let actor_path = root.join("Actor.psc");
        fs::write(
            &actor_path,
            concat!(
                "ScriptName Actor\n",
                "Function Jump(Int Height)\n",
                "EndFunction\n",
                "Weapon Function GetWeapon()\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        let weapon_path = root.join("Weapon.psc");
        fs::write(
            &weapon_path,
            "ScriptName Weapon\nFunction Fire()\nEndFunction\n",
        )
        .unwrap();
        fs::write(
            root.join("Factory.psc"),
            "ScriptName Factory\nActor Function GetActor()\nEndFunction\n",
        )
        .unwrap();
        let project = concat!(
            "ScriptName Project\n",
            "Factory Source\n",
            "Actor[] Targets\n",
            "Function ProjectMember()\n",
            "EndFunction\n",
            "Function Test()\n",
            "  Source.GetActor().\n",
            "  Source.GetActor().Jump(1)\n",
            "  Source.GetActor().GetWeapon().Fire()\n",
            "  (Source.GetActor() As Actor).Jump(2)\n",
            "  (Targets[0]).Jump(3)\n",
            "  Targets[0].\n",
            "  Self.ProjectMember()\n",
            "EndFunction\n",
        );
        let project_path = root.join("Project.psc");
        fs::write(&project_path, project).unwrap();
        let index = WorkspaceIndex::new(&WorkspaceConfig {
            source_roots: vec![root.clone()],
            ..WorkspaceConfig::default()
        })
        .unwrap();
        let project_uri = path_to_file_uri(&project_path).unwrap();

        let call_completion = index.completion(
            &project_uri,
            Position::new(6, "  Source.GetActor().".len() as u32),
        );
        assert!(call_completion.iter().any(|item| item.label == "Jump"));
        assert!(call_completion.iter().any(|item| item.label == "GetWeapon"));
        let array_completion = index.completion(
            &project_uri,
            Position::new(11, "  Targets[0].".len() as u32),
        );
        assert!(array_completion.iter().any(|item| item.label == "Jump"));

        for (line, needle, expected_uri) in [
            (7, "Jump", path_to_file_uri(&actor_path).unwrap()),
            (8, "Fire", path_to_file_uri(&weapon_path).unwrap()),
            (9, "Jump", path_to_file_uri(&actor_path).unwrap()),
            (10, "Jump", path_to_file_uri(&actor_path).unwrap()),
            (12, "ProjectMember", project_uri.clone()),
        ] {
            let line_text = project.lines().nth(line).unwrap();
            let character = line_text.find(needle).unwrap() as u32 + 1;
            assert_eq!(
                index
                    .definition(&project_uri, Position::new(line as u32, character))
                    .unwrap()
                    .uri,
                expected_uri
            );
        }
        let signature_character = project.lines().nth(9).unwrap().find('2').unwrap() as u32;
        let signature = index
            .signature_help(&project_uri, Position::new(9, signature_character))
            .unwrap();
        assert_eq!(signature.signatures[0].label, "Jump(Int Height)");
        let jump_references = index.references(
            &path_to_file_uri(&actor_path).unwrap(),
            Position::new(1, 10),
            false,
        );
        assert_eq!(jump_references.len(), 3);
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "papyrus-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
