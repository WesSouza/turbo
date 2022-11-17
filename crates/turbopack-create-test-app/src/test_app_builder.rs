use std::{
    collections::VecDeque,
    fs::{create_dir_all, File},
    io::prelude::*,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use indoc::{formatdoc, indoc};
use serde_json::json;
use tempfile::TempDir;

fn decide(remaining: usize, min_remaining_decisions: usize) -> bool {
    if remaining == 0 {
        false
    } else if min_remaining_decisions <= remaining {
        true
    } else {
        let urgentness = min_remaining_decisions / remaining;
        (min_remaining_decisions * 11 * 7 * 5) % urgentness == 0
    }
}

fn decide_early(remaining: usize, min_remaining_decisions: usize) -> bool {
    if remaining == 0 {
        false
    } else if min_remaining_decisions <= remaining {
        true
    } else {
        let urgentness = min_remaining_decisions / remaining / remaining;
        (min_remaining_decisions * 11 * 7 * 5) % urgentness == 0
    }
}

fn write_file<P: AsRef<Path>>(name: &str, path: P, content: &[u8]) -> Result<()> {
    File::create(path)
        .with_context(|| format!("creating {name}"))?
        .write_all(content)
        .with_context(|| format!("writing {name}"))
}

#[derive(Debug)]
pub struct TestAppBuilder {
    pub target: Option<PathBuf>,
    pub module_count: usize,
    pub directories_count: usize,
    pub dynamic_import_count: usize,
    pub flatness: usize,
    pub package_json: Option<PackageJsonConfig>,
}

impl Default for TestAppBuilder {
    fn default() -> Self {
        Self {
            target: None,
            module_count: 1000,
            directories_count: 50,
            dynamic_import_count: 0,
            flatness: 5,
            package_json: Some(Default::default()),
        }
    }
}

const SETUP_IMPORTS: &str = indoc! {r#"
import React from "react";
"#};
const SETUP_DETECTOR: &str = indoc! {r#"
let DETECTOR_PROPS = {};
"#};
const SETUP_EVAL: &str = indoc! {r#"
/* @turbopack-bench:eval-start */ 
/* @turbopack-bench:eval-end */
"#};
const DETECTOR_ELEMENT: &str = indoc! {r#"
<Detector {...DETECTOR_PROPS} />
"#};

impl TestAppBuilder {
    pub fn build(&self) -> Result<TestApp> {
        let target = if let Some(target) = self.target.clone() {
            TestAppTarget::Set(target)
        } else {
            TestAppTarget::Temp(tempfile::tempdir().context("creating tempdir")?)
        };
        let path = target.path();
        let mut modules = vec![];
        let src = path.join("src");
        create_dir_all(&src).context("creating src dir")?;

        let mut remaining_modules = self.module_count - 1;
        let mut remaining_directories = self.directories_count;
        let mut remaining_dynamic_imports = self.dynamic_import_count;

        let mut queue = VecDeque::new();
        queue.push_back(src.join("triangle.jsx"));
        remaining_modules -= 1;
        let mut is_root = true;

        let detector_path = src.join("detector.jsx");

        while let Some(file) = queue.pop_front() {
            modules.push(file.clone());

            let relative_detector = if detector_path.parent() == file.parent() {
                "./detector.jsx".to_string()
            } else {
                pathdiff::diff_paths(&detector_path, file.parent().unwrap())
                    .unwrap()
                    .display()
                    .to_string()
            };
            let import_detector = formatdoc! {r#"
                import Detector from "{relative_detector}";
            "#};

            let leaf = remaining_modules == 0
                || (!queue.is_empty()
                    && (queue.len() + remaining_modules) % (self.flatness + 1) == 0);
            if leaf {
                write_file(
                    &format!("leaf file {}", file.display()),
                    &file,
                    formatdoc! {r#"
                            {SETUP_IMPORTS}
                            {import_detector}

                            {SETUP_DETECTOR}
                            {SETUP_EVAL}

                            function Triangle({{ style }}) {{
                                return <>
                                    <polygon points="-5,4.33 0,-4.33 5,4.33" style={{style}} />
                                    {DETECTOR_ELEMENT}
                                </>;
                            }}

                            export default React.memo(Triangle);
                        "#}
                    .as_bytes(),
                )?;
            } else {
                let in_subdirectory = decide(remaining_directories, remaining_modules / 3);

                let import_path;
                let base_file = file.with_extension("");
                let base_file = if in_subdirectory {
                    remaining_directories -= 1;
                    create_dir_all(&base_file).context("creating subdirectory")?;
                    import_path = format!(
                        "./{}/triangle_",
                        base_file.file_name().unwrap().to_str().unwrap()
                    );
                    base_file.join("triangle")
                } else {
                    import_path =
                        format!("./{}_", base_file.file_name().unwrap().to_str().unwrap());
                    base_file
                };

                for i in 1..=3 {
                    let mut f = base_file.clone();
                    f.set_file_name(format!(
                        "{}_{}.jsx",
                        f.file_name().unwrap().to_str().unwrap(),
                        i
                    ));
                    queue.push_back(f);
                }
                remaining_modules = remaining_modules.saturating_sub(3);

                if let [(a, a_), (b, b_), (c, c_)] = &*[("A", "1"), ("B", "2"), ("C", "3")]
                    .into_iter()
                    .enumerate()
                    .map(|(i, (name, n))| {
                        if decide_early(remaining_dynamic_imports, remaining_modules + (2 - i)) {
                            remaining_dynamic_imports -= 1;
                            (
                                format!(
                                    "const {name}Lazy = React.lazy(() => \
                                     import('{import_path}{n}'));"
                                ),
                                format!(
                                    "<React.Suspense><{name}Lazy style={{style}} \
                                     /></React.Suspense>"
                                ),
                            )
                        } else {
                            (
                                format!("import {name} from '{import_path}{n}'"),
                                format!("<{name} style={{style}} />"),
                            )
                        }
                    })
                    .collect::<Vec<_>>()
                {
                    let setup_hydration = if is_root {
                        is_root = false;
                        "\nDETECTOR_PROPS.hydration = true;"
                    } else {
                        ""
                    };
                    write_file(
                        &format!("file with children {}", file.display()),
                        &file,
                        formatdoc! {r#"
                                {SETUP_IMPORTS}
                                {import_detector}
                                {a}
                                {b}
                                {c}

                                {SETUP_DETECTOR}{setup_hydration}
                                {SETUP_EVAL}

                                function Container({{ style }}) {{
                                    return <>
                                        <g transform="translate(0 -2.16)   scale(0.5 0.5)">
                                            {a_}
                                        </g>
                                        <g transform="translate(-2.5 2.16) scale(0.5 0.5)">
                                            {b_}
                                        </g>
                                        <g transform="translate(2.5 2.16)  scale(0.5 0.5)">
                                            {c_}
                                        </g>
                                        {DETECTOR_ELEMENT}
                                    </>;
                                }}

                                export default React.memo(Container);
                            "#}
                        .as_bytes(),
                    )?;
                } else {
                    unreachable!()
                }
            }
        }

        let bootstrap = indoc! {r#"
            import React from "react";
            import { createRoot } from "react-dom/client";
            import Triangle from "./triangle.jsx";

            function App() {
                return <svg height="100%" viewBox="-5 -4.33 10 8.66" style={{ }}>
                    <Triangle style={{ fill: "white" }}/>
                </svg>
            }

            document.body.style.backgroundColor = "black";
            let root = document.createElement("main");
            document.body.appendChild(root);
            createRoot(root).render(<App />);
        "#};
        write_file(
            "bootstrap file",
            src.join("index.jsx"),
            bootstrap.as_bytes(),
        )?;

        let pages = src.join("pages");
        create_dir_all(&pages)?;

        // The page is e. g. used by Next.js
        let bootstrap_page = indoc! {r#"
            import React from "react";
            import Triangle from "../triangle.jsx";

            export default function Page() {
                return <svg height="100%" viewBox="-5 -4.33 10 8.66" style={{ backgroundColor: "black" }}>
                    <Triangle style={{ fill: "white" }}/>
                </svg>
            }
        "#};
        write_file(
            "bootstrap page",
            pages.join("page.jsx"),
            bootstrap_page.as_bytes(),
        )?;

        // The page is e. g. used by Next.js
        let bootstrap_static_page = indoc! {r#"
            import React from "react";
            import Triangle from "../triangle.jsx";

            export default function Page() {
                return <svg height="100%" viewBox="-5 -4.33 10 8.66" style={{ backgroundColor: "black" }}>
                    <Triangle style={{ fill: "white" }}/>
                </svg>
            }

            export function getStaticProps() {
                return {
                    props: {}
                };
            }
        "#};
        write_file(
            "bootstrap static page",
            pages.join("static.jsx"),
            bootstrap_static_page.as_bytes(),
        )?;

        let app_dir = src.join("app");
        create_dir_all(app_dir.join("app"))?;
        create_dir_all(app_dir.join("client"))?;

        // The page is e. g. used by Next.js
        let bootstrap_app_page = indoc! {r#"
            import React from "react";
            import Triangle from "../../triangle.jsx";

            export default function Page() {
                return <svg height="100%" viewBox="-5 -4.33 10 8.66" style={{ backgroundColor: "black" }}>
                    <Triangle style={{ fill: "white" }}/>
                </svg>
            }
        "#};
        write_file(
            "bootstrap app page",
            app_dir.join("page.jsx"),
            bootstrap_app_page.as_bytes(),
        )?;

        // The component is used to measure hydration and commit time for app/page.jsx
        let detector_component = indoc! {r#"
            "use client";

            import React from "react";

            export default function Detector({ message, hydration }) {
                React.useEffect(() => {
                    if (hydration) {
                        globalThis.__turbopackBenchBinding && globalThis.__turbopackBenchBinding("Hydration done");
                    }
                    if (message) {
                        globalThis.__turbopackBenchBinding && globalThis.__turbopackBenchBinding(message);
                    }
                }, [message, hydration]);
                return null;
            }
        "#};
        write_file(
            "detector component",
            app_dir.join("client/detector.jsx"),
            detector_component.as_bytes(),
        )?;

        // The page is e. g. used by Next.js
        let bootstrap_app_client_page = indoc! {r#"
            "use client";
            import React from "react";
            import Triangle from "../../triangle.jsx";

            export default function Page() {
                return <svg height="100%" viewBox="-5 -4.33 10 8.66" style={{ backgroundColor: "black" }}>
                    <Triangle style={{ fill: "white" }}/>
                </svg>
            }
        "#};
        write_file(
            "bootstrap app client page",
            app_dir.join("client/page.jsx"),
            bootstrap_app_client_page.as_bytes(),
        )?;

        // This root layout is e. g. used by Next.js
        let bootstrap_layout = indoc! {r#"
            export default function RootLayout({ children }) {
                return (
                    <html lang="en">
                        <head>
                            <meta charSet="UTF-8" />
                            <meta name="viewport" content="width=device-width, initial-scale=1.0" />
                            <title>Turbopack Test App</title>
                        </head>
                        <body>
                            {children}
                        </body>
                    </html>
                );
            }
        "#};
        write_file(
            "bootstrap layout",
            src.join("layout.jsx"),
            bootstrap_layout.as_bytes(),
        )?;

        // This HTML is used e. g. by Vite
        let bootstrap_html = indoc! {r#"
            <!DOCTYPE html>
            <html lang="en">
                <head>
                    <meta charset="UTF-8" />
                    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
                    <title>Turbopack Test App</title>
                </head>
                <body>
                    <script type="module" src="/src/index.jsx"></script>
                </body>
            </html>
        "#};
        write_file(
            "bootstrap html in root",
            src.join("index.html"),
            bootstrap_html.as_bytes(),
        )?;

        // This HTML is used e. g. by webpack
        let bootstrap_html2 = indoc! {r#"
            <!DOCTYPE html>
            <html lang="en">
                <head>
                    <meta charset="UTF-8" />
                    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
                    <title>Turbopack Test App</title>
                </head>
                <body>
                    <script src="main.js"></script>
                </body>
            </html>
        "#};

        let public = path.join("public");
        create_dir_all(&public).context("creating public dir")?;

        write_file(
            "bootstrap html",
            public.join("index.html"),
            bootstrap_html2.as_bytes(),
        )?;

        write_file(
            "vite node.js server",
            path.join("vite-server.mjs"),
            include_bytes!("templates/vite-server.mjs"),
        )?;
        write_file(
            "vite server entry",
            path.join("src/vite-entry-server.jsx"),
            include_bytes!("templates/vite-entry-server.jsx"),
        )?;
        write_file(
            "vite client entry",
            path.join("src/vite-entry-client.jsx"),
            include_bytes!("templates/vite-entry-client.jsx"),
        )?;

        if let Some(package_json) = &self.package_json {
            // These dependencies are needed
            let package_json = json!({
                "name": "turbopack-test-app",
                "private": true,
                "version": "0.0.0",
                "dependencies": {
                    "react": package_json.react_version.clone(),
                    "react-dom": package_json.react_version.clone(),
                }
            });
            write_file(
                "package.json",
                path.join("package.json"),
                format!("{:#}", package_json).as_bytes(),
            )?;
        }

        Ok(TestApp { target, modules })
    }
}

/// Configuration struct to generate the `package.json` file of the test app.
#[derive(Debug)]
pub struct PackageJsonConfig {
    /// The version of React to use.
    pub react_version: String,
}

impl Default for PackageJsonConfig {
    fn default() -> Self {
        Self {
            react_version: "^18.2.0".to_string(),
        }
    }
}

#[derive(Debug)]
enum TestAppTarget {
    Set(PathBuf),
    Temp(TempDir),
}

impl TestAppTarget {
    /// Returns the path to the directory containing the app.
    fn path(&self) -> &Path {
        match &self {
            TestAppTarget::Set(target) => target.as_path(),
            TestAppTarget::Temp(target) => target.path(),
        }
    }
}

#[derive(Debug)]
pub struct TestApp {
    target: TestAppTarget,
    modules: Vec<PathBuf>,
}

impl TestApp {
    /// Returns the path to the directory containing the app.
    pub fn path(&self) -> &Path {
        self.target.path()
    }

    /// Returns the list of modules in this app.
    pub fn modules(&self) -> &[PathBuf] {
        &self.modules
    }
}
