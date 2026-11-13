#![cfg(feature = "lang-php")]
//! End-to-end checks of PHP linking: a small project is written to a temporary directory,
//! indexed, and asked through the command line, the way a harness would.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_codefast");

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Project {
        let root = std::env::temp_dir().join(format!("codefast-php-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in files {
            let p = root.join("src").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
        }
        Project { root }
    }

    fn ask(&self, args: &[&str]) -> String {
        let out = Command::new(BIN)
            .arg("--root")
            .arg(self.root.join("src"))
            .args(args)
            .env("CODEFAST_INDEX_DIR", self.root.join("index"))
            .output()
            .expect("run codefast");
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn has(text: &str, needle: &str) {
    assert!(text.contains(needle), "expected `{needle}` in:\n{text}");
}

fn lacks(text: &str, needle: &str) {
    assert!(!text.contains(needle), "did not expect `{needle}` in:\n{text}");
}

/// A small Laravel-shaped application: models with a base class, an interface, a trait, a
/// namespaced helper, a global helper, a service and a test.
fn app(name: &str) -> Project {
    Project::new(
        name,
        &[
            (
                "app/Models/Model.php",
                "<?php\nnamespace App\\Models;\n\nabstract class Model\n{\n    public function save(): bool\n    {\n        return true;\n    }\n\n    public static function find(int $id): static\n    {\n        return new static();\n    }\n}\n",
            ),
            (
                "app/Models/User.php",
                "<?php\nnamespace App\\Models;\n\nuse App\\Contracts\\HasName;\nuse App\\Concerns\\Greets;\n\nclass User extends Model implements HasName\n{\n    use Greets;\n\n    public string $email = '';\n\n    public function __construct(public Profile $profile)\n    {\n    }\n\n    public function name(): string\n    {\n        return $this->email;\n    }\n\n    public function save(): bool\n    {\n        $this->touch();\n        return parent::save();\n    }\n\n    private function touch(): void\n    {\n    }\n}\n",
            ),
            (
                "app/Models/Profile.php",
                "<?php\nnamespace App\\Models;\n\nclass Profile\n{\n    public function avatar(): string\n    {\n        return '';\n    }\n}\n",
            ),
            ("app/Contracts/HasName.php", "<?php\nnamespace App\\Contracts;\n\ninterface HasName\n{\n    public function name(): string;\n}\n"),
            (
                "app/Concerns/Greets.php",
                "<?php\nnamespace App\\Concerns;\n\ntrait Greets\n{\n    public function greet(): string\n    {\n        return 'hi ' . $this->name();\n    }\n}\n",
            ),
            ("app/Support/helpers.php", "<?php\nnamespace App\\Support;\n\nfunction format_name(string $n): string\n{\n    return trim($n);\n}\n"),
            ("app/helpers.php", "<?php\n\nfunction app_path(string $p = ''): string\n{\n    return $p;\n}\n"),
            (
                "app/Services/UserService.php",
                "<?php\nnamespace App\\Services;\n\nuse App\\Models\\User;\nuse App\\Contracts\\HasName;\nuse function App\\Support\\format_name;\n\nclass UserService\n{\n    public function __construct(private User $user)\n    {\n    }\n\n    public function run(HasName $named): string\n    {\n        $found = User::find(1);\n        $found->save();\n        $this->user->save();\n        $this->user->profile->avatar();\n        $this->user->greet();\n        $this->audit();\n        app_path('x');\n        return format_name($named->name());\n    }\n\n    protected function audit(): void\n    {\n    }\n}\n",
            ),
            (
                "tests/UserServiceTest.php",
                "<?php\nnamespace Tests;\n\nuse App\\Services\\UserService;\nuse App\\Models\\User;\n\nclass UserServiceTest extends TestCase\n{\n    public function testRun(): void\n    {\n        $service = new UserService(User::find(2));\n        $service->run(User::find(3));\n    }\n}\n",
            ),
        ],
    )
}

#[test]
fn php_definitions_are_found_by_name() {
    let p = app("php_definitions_are_found_by_name");
    let found = p.ask(&["find", "User"]);
    has(&found, "app/Models/User.php:7 class User extends Model implements HasName");
    has(&p.ask(&["show", "User.save"]), "return parent::save();");
    // A promoted constructor parameter is a field of the class.
    has(&p.ask(&["find", "profile"]), "public Profile $profile  (in User)");
}

#[test]
fn php_callers_through_typed_receivers_and_this() {
    let p = app("php_callers_through_typed_receivers_and_this");
    // `$this->user` is a typed property; `$found` comes from `find()` returning `static`.
    let callers = p.ask(&["callers", "User.save"]);
    has(&callers, "18 run: $this->user->save();");
    // A property read through another property, across files.
    has(&p.ask(&["callers", "Profile.avatar"]), "19 run: $this->user->profile->avatar();");
    // `$this->m()` inside the class.
    has(&p.ask(&["callers", "UserService.audit"]), "21 run: $this->audit();");
    has(&p.ask(&["callers", "User.touch"]), "24 save: $this->touch();");
}

#[test]
fn php_interface_dispatch() {
    let p = app("php_interface_dispatch");
    let callers = p.ask(&["callers", "HasName.name"]);
    has(&callers, "23 run: return format_name($named->name());");
    // The call through the interface reaches the implementation.
    has(&p.ask(&["callees", "run"]), "[HasName.name] → 1 impls: User.name");
}

#[test]
fn php_inheritance_and_parent_calls() {
    let p = app("php_inheritance_and_parent_calls");
    let callers = p.ask(&["callers", "Model.save"]);
    has(&callers, "25 save: return parent::save();");
    has(&callers, "17 run: $found->save();");
    // `parent::save()` runs the parent's method, never the override it is written in.
    lacks(&p.ask(&["callers", "User.save"]), "parent::save()");
    has(&p.ask(&["refs", "Model"]), "class User extends Model implements HasName");
}

#[test]
fn php_static_calls() {
    let p = app("php_static_calls");
    let callers = p.ask(&["callers", "Model.find"]);
    has(&callers, "16 run: $found = User::find(1);");
    // `new T(..)` runs the constructor of `T`, named `__construct`.
    has(&p.ask(&["callers", "UserService.__construct", "--tests"]), "11 testRun: $service = new UserService(User::find(2));");
}

#[test]
fn php_trait_methods_belong_to_the_classes_using_them() {
    let p = app("php_trait_methods_belong_to_the_classes_using_them");
    has(&p.ask(&["callers", "Greets.greet"]), "20 run: $this->user->greet();");
    has(&p.ask(&["refs", "Greets"]), "use Greets;");
}

#[test]
fn php_namespaced_and_global_functions() {
    let p = app("php_namespaced_and_global_functions");
    // `use function App\Support\format_name;`
    let callers = p.ask(&["callers", "format_name"]);
    has(&callers, "23 run: return format_name($named->name());");
    has(&callers, "all 1 call sites");
    // A function of the global namespace called from a namespace: PHP falls back to it.
    has(&p.ask(&["callers", "app_path"]), "22 run: app_path('x');");
}

#[test]
fn php_properties_across_files() {
    let p = app("php_properties_across_files");
    let refs = p.ask(&["refs", "User.profile"]);
    has(&refs, "app/Services/UserService.php");
    has(&refs, "19 run: $this->user->profile->avatar();");
    has(&p.ask(&["refs", "User.email"]), "return $this->email;");
}

#[test]
fn php_test_methods_are_test_code() {
    let p = app("php_test_methods_are_test_code");
    // Without --tests the call from the test is named apart, without its line. With it, it is
    // listed like any other.
    let plain = p.ask(&["callers", "UserService.run"]);
    has(&plain, "tests/UserServiceTest.php: testRun:12");
    lacks(&plain, "$service->run(User::find(3));");
    has(&p.ask(&["callers", "UserService.run", "--tests"]), "12 testRun: $service->run(User::find(3));");
}

#[test]
fn php_callables_and_imports_by_alias() {
    let p = Project::new(
        "callables",
        &[
            (
                "src/Http/Controller.php",
                "<?php\nnamespace Acme\\Http;\n\nuse Acme\\Domain\\Order as Purchase;\n\nclass Controller\n{\n    public function boot(Router $router): void\n    {\n        $router->get('/x', [$this, 'show']);\n        $router->get('/y', [Purchase::class, 'make']);\n        $this->handler = $this->show(...);\n        $order = new Purchase();\n        $order->ship();\n    }\n\n    public function show(): void\n    {\n    }\n}\n",
            ),
            (
                "src/Domain/Order.php",
                "<?php\nnamespace Acme\\Domain;\n\nclass Order\n{\n    public static function make(): static\n    {\n        return new static();\n    }\n\n    public function ship(): void\n    {\n    }\n}\n",
            ),
            ("src/Http/Router.php", "<?php\nnamespace Acme\\Http;\n\nclass Router\n{\n    public function get(string $uri, $action): void\n    {\n    }\n}\n"),
        ],
    );
    // Callables in the forms that name a method exactly.
    let show = p.ask(&["callers", "Controller.show"]);
    has(&show, "to get() at src/Http/Controller.php:10");
    has(&p.ask(&["callers", "Order.make"]), "to get() at src/Http/Controller.php:11");
    // A class imported under another name.
    has(&p.ask(&["callers", "Order.ship"]), "14 boot: $order->ship();");
    has(&p.ask(&["refs", "Order"]), "13 boot: $order = new Purchase();");
    // `Router $router` is in the same namespace without an import.
    has(&p.ask(&["callers", "Router.get"]), "10 boot: $router->get('/x', [$this, 'show']);");
}

#[test]
fn php_templates_mixing_html_and_code() {
    let p = Project::new(
        "templates",
        &[
            ("theme/single.php", "<html>\n<body>\n<?php if (have_posts()): ?>\n  <h1><?= render_title($post) ?></h1>\n<?php endif; ?>\n</body>\n</html>\n"),
            ("theme/functions.php", "<?php\nfunction render_title($post) { return esc($post); }\nfunction have_posts() { return true; }\n"),
        ],
    );
    has(&p.ask(&["callers", "render_title"]), "4 <file>: <h1><?= render_title($post) ?></h1>");
    has(&p.ask(&["callers", "have_posts"]), "3 <file>: <?php if (have_posts()): ?>");
}

#[test]
fn php_test_files_are_known_by_their_name() {
    // PHPUnit classes end in `Test`; a file named so is test code wherever it lives.
    let p = Project::new(
        "test-names",
        &[
            ("lib/Billing.php", "<?php\nnamespace Shop;\n\nclass Billing\n{\n    public function charge(): void\n    {\n    }\n}\n"),
            (
                "lib/BillingTest.php",
                "<?php\nnamespace Shop;\n\nclass BillingTest\n{\n    public function testCharge(): void\n    {\n        (new Billing())->charge();\n    }\n}\n",
            ),
        ],
    );
    // A call from test code is listed apart, in the compact form, and with its line on request.
    let plain = p.ask(&["callers", "Billing.charge"]);
    has(&plain, "(all 0 call sites, 0 files, 1 more in tests)");
    has(&plain, "lib/BillingTest.php: testCharge:8");
    lacks(&plain, "(new Billing())->charge();");
    has(&p.ask(&["callers", "Billing.charge", "--tests"]), "8 testCharge: (new Billing())->charge();");
}
