mod testkit;
#[test]
fn testkit_builds() {
    let (_p, rec) = testkit::ScriptedProvider::new(vec![testkit::assistant_text("hi")]);
    assert_eq!(rec.lock().unwrap().len(), 0);
}
