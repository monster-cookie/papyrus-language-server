use crate::semantic::{Declaration, DeclarationKind, SemanticExpression, TypeRef};

use super::{IndexedDocument, WorkspaceIndex};

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
}

enum ExpressionResolution<'a> {
    Declaration(&'a Declaration),
    Script(&'a IndexedDocument, &'a Declaration),
    Value(TypeRef),
}

pub(super) fn resolve_member_expression<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    receiver: &SemanticExpression,
    member: &str,
) -> Option<&'a Declaration> {
    let receiver = resolve_expression(context, current, receiver, 0)?;
    resolve_member_from_resolution(context, current, receiver, member)
}

pub(super) fn members_for_expression<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    expression: &SemanticExpression,
) -> Vec<&'a Declaration> {
    match resolve_expression(context, current, expression, 0) {
        Some(ExpressionResolution::Declaration(declaration)) => declaration
            .ty
            .as_ref()
            .and_then(TypeRef::scalar_name)
            .map(|ty| context.members_of_type(current, ty))
            .unwrap_or_default(),
        Some(ExpressionResolution::Script(_, script)) => context.members_of(script),
        Some(ExpressionResolution::Value(ty)) => ty
            .scalar_name()
            .map(|ty| context.members_of_type(current, ty))
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

fn resolve_expression<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    expression: &SemanticExpression,
    depth: usize,
) -> Option<ExpressionResolution<'a>> {
    if depth >= MAX_EXPRESSION_DEPTH {
        return None;
    }
    match expression {
        SemanticExpression::Identifier { name, byte_offset } => {
            if name.eq_ignore_ascii_case("self") {
                let script_name = current.semantic.script_name.as_deref()?;
                let effective_name = context
                    .unique_script(script_name)
                    .map(|(_, script)| context.declaration_name(script))
                    .unwrap_or(script_name);
                return Some(ExpressionResolution::Value(TypeRef {
                    name: effective_name.to_owned(),
                    array: false,
                }));
            }
            if let Some(declaration) = context.resolve_visible_name(current, name, *byte_offset) {
                if declaration.kind == DeclarationKind::Script {
                    let (document, script) = context.unique_script(&declaration.name)?;
                    return Some(ExpressionResolution::Script(document, script));
                }
                return Some(ExpressionResolution::Declaration(declaration));
            }
            context
                .unique_script(name)
                .map(|(document, script)| ExpressionResolution::Script(document, script))
        }
        SemanticExpression::Member {
            object,
            member,
            byte_offset: _,
        } => {
            let receiver = resolve_expression(context, current, object, depth + 1)?;
            resolve_member_from_resolution(context, current, receiver, member)
                .map(ExpressionResolution::Declaration)
        }
        SemanticExpression::Call { function } => {
            let ExpressionResolution::Declaration(function) =
                resolve_expression(context, current, function, depth + 1)?
            else {
                return None;
            };
            if !matches!(
                function.kind,
                DeclarationKind::Function | DeclarationKind::Event
            ) {
                return None;
            }
            function.ty.clone().map(ExpressionResolution::Value)
        }
        SemanticExpression::Subscript { array } => {
            let mut ty = expression_type(context, current, array, depth + 1)?;
            if !ty.array {
                return None;
            }
            ty.array = false;
            Some(ExpressionResolution::Value(ty))
        }
        SemanticExpression::Cast { ty } | SemanticExpression::New { ty } => {
            Some(ExpressionResolution::Value(ty.clone()))
        }
        SemanticExpression::Parenthesized { value } => {
            resolve_expression(context, current, value, depth + 1)
        }
    }
}

fn expression_type<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    expression: &SemanticExpression,
    depth: usize,
) -> Option<TypeRef> {
    match resolve_expression(context, current, expression, depth)? {
        ExpressionResolution::Declaration(declaration) => declaration.ty.clone(),
        ExpressionResolution::Script(_, script) => Some(TypeRef {
            name: context.declaration_name(script).to_owned(),
            array: false,
        }),
        ExpressionResolution::Value(ty) => Some(ty),
    }
}

fn resolve_member_from_resolution<'a>(
    context: &'a impl ExpressionContext,
    current: &'a IndexedDocument,
    receiver: ExpressionResolution<'a>,
    member: &str,
) -> Option<&'a Declaration> {
    match receiver {
        ExpressionResolution::Declaration(declaration) => {
            let ty = declaration.ty.as_ref()?.scalar_name()?;
            unique_named(context.members_of_type(current, ty), member, context)
        }
        ExpressionResolution::Value(ty) => {
            let ty = ty.scalar_name()?;
            unique_named(context.members_of_type(current, ty), member, context)
        }
        ExpressionResolution::Script(document, script) => unique_named(
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

fn unique_named<'a>(
    declarations: Vec<&'a Declaration>,
    name: &str,
    context: &'a impl ExpressionContext,
) -> Option<&'a Declaration> {
    let mut matches = declarations
        .into_iter()
        .filter(|declaration| context.name_matches(declaration, name));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
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
