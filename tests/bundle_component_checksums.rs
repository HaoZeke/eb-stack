use eb_stack::{packaging_gate, resolve_easyconfig_str};

fn recipe(body: &str) -> String {
    format!(
        "easyblock = 'Bundle'\nname = 'RenderBundle'\nversion = '1.0'\n\
         toolchain = {{'name': 'GCC', 'version': '15.2.0'}}\nmoduleclass = 'lib'\n{body}"
    )
}

#[test]
fn complete_component_archives_pass_without_inventing_root_checksums() {
    let source = recipe(&format!(
        "components = [('Renderer', '1.0', {{'sources': ['render.tar.xz'], \
         'checksums': ['{}']}}), ('Data', '2.0', {{'sources': ['data.tar.gz'], \
         'checksums': ['{}']}})]\n",
        "a".repeat(64),
        "b".repeat(64)
    ));
    let resolved = resolve_easyconfig_str(&source).unwrap();
    assert!(resolved.checksums.is_empty());
    assert!(resolved.component_checksums_complete);
    packaging_gate(&resolved, &[]).expect("all component sources carry checksums");
}

#[test]
fn inherited_component_sources_and_checksums_pass() {
    let source = recipe(&format!(
        "default_component_specs = {{'sources': [SOURCE_TAR_GZ], 'checksums': ['{}']}}\n\
         components = [('Renderer', '1.0'), ('Data', '2.0')]\n",
        "c".repeat(64)
    ));
    let resolved = resolve_easyconfig_str(&source).unwrap();
    assert!(resolved.component_checksums_complete);
    packaging_gate(&resolved, &[]).expect("each component inherits its source checksum");
}

fn rejects(body: &str) {
    let resolved = resolve_easyconfig_str(&recipe(body)).unwrap();
    assert!(
        !resolved.component_checksums_complete,
        "component checksums should be incomplete: {body}"
    );
    let errs = packaging_gate(&resolved, &[]).expect_err(body);
    assert!(
        errs.iter().any(|err| err == "missing checksums"),
        "{body}: {errs:?}"
    );
}

#[test]
fn one_unchecked_component_refuses_the_bundle() {
    rejects(&format!(
        "components = [('Renderer', '1.0', {{'sources': ['render.tar.gz'], \
         'checksums': ['{}']}}), ('Data', '2.0', {{'sources': ['data.tar.gz']}})]\n",
        "a".repeat(64)
    ));
}

#[test]
fn empty_component_checksum_is_not_coverage() {
    rejects(
        "components = [('Renderer', '1.0', {'sources': ['render.tar.gz'], 'checksums': ['']})]\n",
    );
}

#[test]
fn a_patch_needs_its_own_checksum_entry() {
    rejects(&format!(
        "components = [('Renderer', '1.0', {{'sources': ['render.tar.gz'], \
         'patches': ['render.patch'], 'checksums': ['{}']}})]\n",
        "a".repeat(64)
    ));
}

#[test]
fn a_postinstall_patch_needs_its_own_checksum_entry() {
    rejects(&format!(
        "components = [('Renderer', '1.0', {{'sources': ['render.tar.gz'], \
         'postinstallpatches': ['render-post.patch'], 'checksums': ['{}']}})]\n",
        "a".repeat(64)
    ));
}

#[test]
fn an_alternate_postinstall_patch_name_needs_its_own_checksum_entry() {
    rejects(&format!(
        "components = [('Renderer', '1.0', {{'sources': ['render.tar.gz'], \
         'post_install_patches': ['render-post.patch'], 'checksums': ['{}']}})]\n",
        "a".repeat(64)
    ));
}

#[test]
fn postinstall_patches_pass_when_their_checksum_is_present() {
    let source = recipe(&format!(
        "components = [('Renderer', '1.0', {{'sources': ['render.tar.gz'], \
         'patches': ['render.patch'], 'postinstallpatches': ['render-post.patch'], \
         'checksums': ['{}', '{}', '{}']}})]\n",
        "a".repeat(64),
        "d".repeat(64),
        "e".repeat(64)
    ));
    let resolved = resolve_easyconfig_str(&source).unwrap();
    assert!(resolved.checksums.is_empty());
    assert!(resolved.component_checksums_complete);
    packaging_gate(&resolved, &[])
        .expect("source, patch, and postinstall patch each have a checksum");
}

#[test]
fn malformed_component_cannot_disappear_from_coverage() {
    rejects(&format!(
        "components = [('Renderer', '1.0', {{'sources': ['render.tar.gz'], \
         'checksums': ['{}']}}), 'unparsed-component']\n",
        "a".repeat(64)
    ));
}

#[test]
fn component_hashes_cannot_cover_an_unchecked_root_archive() {
    let source = recipe(&format!(
        "sources = ['bundle.tar.gz']\ncomponents = [('Renderer', '1.0', \
         {{'sources': ['render.tar.gz'], 'checksums': ['{}']}})]\n",
        "a".repeat(64)
    ));
    let resolved = resolve_easyconfig_str(&source).unwrap();
    assert!(resolved.component_checksums_complete);
    assert_eq!(resolved.sources_count, 1);
    assert!(resolved.checksums.is_empty());
    let errs = packaging_gate(&resolved, &[]).expect_err("unchecked root archive");
    assert!(
        errs.iter().any(|err| err == "missing checksums"),
        "{errs:?}"
    );
}

#[test]
fn a_skipped_component_mutation_refuses_partial_coverage() {
    rejects(&format!(
        "components = [('Renderer', '1.0', {{'sources': ['render.tar.gz'], \
         'checksums': ['{}']}})]\ncomponents += unsupported_components()\n",
        "a".repeat(64)
    ));
}
