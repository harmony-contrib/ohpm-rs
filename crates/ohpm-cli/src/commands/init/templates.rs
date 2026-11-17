//! File contents for the `init` scaffolds.
//!
//! The layouts mirror the DevEco Studio 26 "New Project", "Static Library",
//! and "Shared Library" templates (stage model, ArkTS). Placeholder icons are
//! generated at runtime by [`super::png`].

use super::png;
use super::scaffold::{InitKind, Prepared, Sdk};

pub const GITIGNORE: &str = r#"/node_modules
/oh_modules
/local.properties
/.idea
**/build
/.hvigor
.cxx
/.clangd
/.clang-format
/.clang-tidy
**/.test
/.appanalyzer
"#;

pub const MODULE_GITIGNORE: &str = r#"/node_modules
/oh_modules
/.preview
/build
/.cxx
/.test
"#;

pub const CODE_LINTER: &str = r#"{
  "files": [
    "**/*.ets"
  ],
  "ignore": [
    "**/src/ohosTest/**/*",
    "**/src/test/**/*",
    "**/src/mock/**/*",
    "**/node_modules/**/*",
    "**/oh_modules/**/*",
    "**/build/**/*",
    "**/.preview/**/*"
  ],
  "ruleSet": [
    "plugin:@performance/recommended",
    "plugin:@typescript-eslint/recommended"
  ],
  "rules": {
    "@security/no-unsafe-aes": "error",
    "@security/no-unsafe-hash": "error",
    "@security/no-unsafe-mac": "warn",
    "@security/no-unsafe-dh": "error",
    "@security/no-unsafe-dsa": "error",
    "@security/no-unsafe-ecdsa": "error",
    "@security/no-unsafe-rsa-encrypt": "error",
    "@security/no-unsafe-rsa-sign": "error",
    "@security/no-unsafe-rsa-key": "error",
    "@security/no-unsafe-dsa-key": "error",
    "@security/no-unsafe-dh-key": "error",
    "@security/no-unsafe-3des": "error"
  }
}
"#;

pub const HVIGORFILE_APP: &str = r#"import { appTasks } from '@ohos/hvigor-ohos-plugin';

export default {
  system: appTasks, /* Built-in plugin of Hvigor. It cannot be modified. */
  plugins: []       /* Custom plugin to extend the functionality of Hvigor. */
}
"#;

pub const HVIGORFILE_HAP: &str = r#"import { hapTasks } from '@ohos/hvigor-ohos-plugin';

export default {
  system: hapTasks, /* Built-in plugin of Hvigor. It cannot be modified. */
  plugins: []       /* Custom plugin to extend the functionality of Hvigor. */
}
"#;

pub const HVIGORFILE_HAR: &str = r#"import { harTasks } from '@ohos/hvigor-ohos-plugin';

export default {
  system: harTasks, /* Built-in plugin of Hvigor. It cannot be modified. */
  plugins: []       /* Custom plugin to extend the functionality of Hvigor. */
}
"#;

pub const HVIGORFILE_HSP: &str = r#"import { hspTasks } from '@ohos/hvigor-ohos-plugin';

export default {
  system: hspTasks, /* Built-in plugin of Hvigor. It cannot be modified. */
  plugins: []       /* Custom plugin to extend the functionality of Hvigor. */
}
"#;

pub const LAYERED_IMAGE: &str = r#"{
  "layered-image": {
    "background": "$media:background",
    "foreground": "$media:foreground"
  }
}
"#;

pub const ENTRY_OH_PACKAGE: &str = r#"{
  "name": "entry",
  "version": "1.0.0",
  "description": "Please describe the basic information.",
  "main": "",
  "author": "",
  "license": "",
  "dependencies": {}
}
"#;

pub const HAR_INDEX: &str = "export { MainPage } from './src/main/ets/components/MainPage';\n";

pub const HSP_INDEX: &str = "export { add } from './src/main/ets/utils/Calc';\n";

pub const CALC: &str = r#"export function add(a: number, b: number) {
  return a + b;
}
"#;

pub const MAIN_PAGE: &str = r#"@Component
export struct MainPage {
  @State message: string = 'Hello World';

  build() {
    Row() {
      Column() {
        Text(this.message)
          .fontSize($r('app.float.page_text_font_size'))
          .fontWeight(FontWeight.Bold)
          .onClick(() => {
            this.message = 'Welcome';
          })
      }
      .width('100%')
    }
    .height('100%')
  }
}
"#;

pub const ENTRY_BACKUP_ABILITY: &str = r#"import { hilog } from '@kit.PerformanceAnalysisKit';
import { BackupExtensionAbility, BundleVersion } from '@kit.CoreFileKit';

const DOMAIN = 0x0000;

export default class EntryBackupAbility extends BackupExtensionAbility {
  async onBackup() {
    hilog.info(DOMAIN, 'testTag', 'onBackup ok');
    await Promise.resolve();
  }

  async onRestore(bundleVersion: BundleVersion) {
    hilog.info(DOMAIN, 'testTag', 'onRestore ok %{public}s', JSON.stringify(bundleVersion));
    await Promise.resolve();
  }
}
"#;

pub const MAIN_PAGES: &str = r#"{
  "src": [
    "pages/Index"
  ]
}
"#;

pub const FLOAT_JSON: &str = r#"{
  "float": [
    {
      "name": "page_text_font_size",
      "value": "50fp"
    }
  ]
}
"#;

pub const HAR_STRING_JSON: &str = r#"{
  "string": [
    {
      "name": "page_show",
      "value": "page from package"
    }
  ]
}
"#;

pub const HSP_STRING_JSON: &str = r#"{
  "string": [
    {
      "name": "shared_desc",
      "value": "description"
    }
  ]
}
"#;

pub const MOCK_CONFIG: &str = "{\n}\n";

pub const OBFUSCATION_RULES: &str = r#"# Define project specific obfuscation rules here.
# You can include the obfuscation configuration files in the current module's build-profile.json5.
#
# For more details, see
#   https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/source-obfuscation

# Obfuscation options:
# -disable-obfuscation: disable all obfuscations
# -enable-property-obfuscation: obfuscate the property names
# -enable-toplevel-obfuscation: obfuscate the names in the global scope
# -compact: remove unnecessary blank spaces and all line feeds
# -remove-log: remove all console.* statements
# -print-namecache: print the name cache that contains the mapping from the old names to new names
# -apply-namecache: reuse the given cache file

# Keep options:
# -keep-property-name: specifies property names that you want to keep
# -keep-global-name: specifies names that you want to keep in the global scope

-enable-property-obfuscation
-enable-toplevel-obfuscation
-enable-filename-obfuscation
-enable-export-obfuscation
"#;

pub const CONSUMER_RULES: &str = "";

pub const ABILITY_TEST: &str = r#"import { hilog } from '@kit.PerformanceAnalysisKit';
import { describe, beforeAll, beforeEach, afterEach, afterAll, it, expect } from '@ohos/hypium';

export default function abilityTest() {
  describe('ActsAbilityTest', () => {
    // Defines a test suite. Two parameters are supported: test suite name and test suite function.
    beforeAll(() => {
      // Presets an action, which is performed only once before all test cases of the test suite start.
      // This API supports only one parameter: preset action function.
    })
    beforeEach(() => {
      // Presets an action, which is performed before each unit test case starts.
      // The number of execution times is the same as the number of test cases defined by **it**.
      // This API supports only one parameter: preset action function.
    })
    afterEach(() => {
      // Presets a clear action, which is performed after each unit test case ends.
      // The number of execution times is the same as the number of test cases defined by **it**.
      // This API supports only one parameter: clear action function.
    })
    afterAll(() => {
      // Presets a clear action, which is performed after all test cases of the test suite end.
      // This API supports only one parameter: clear action function.
    })
    it('assertContain', 0, () => {
      // Defines a test case. This API supports three parameters: test case name, filter parameter, and test case function.
      hilog.info(0x0000, 'testTag', '%{public}s', 'it begin');
      let a = 'abc';
      let b = 'b';
      // Defines a variety of assertion methods, which are used to declare expected boolean conditions.
      expect(a).assertContain(b);
      expect(a).assertEqual(a);
    })
  })
}
"#;

pub const OHOS_TEST_LIST: &str = r#"import abilityTest from './Ability.test';

export default function testsuite() {
  abilityTest();
}
"#;

pub const UNIT_TEST_LIST: &str = r#"import localUnitTest from './LocalUnit.test';

export default function testsuite() {
  localUnitTest();
}
"#;

pub const LOCAL_UNIT_TEST: &str = r#"import { describe, beforeAll, beforeEach, afterEach, afterAll, it, expect } from '@ohos/hypium';

export default function localUnitTest() {
  describe('localUnitTest', () => {
    // Defines a test suite. Two parameters are supported: test suite name and test suite function.
    beforeAll(() => {
      // Presets an action, which is performed only once before all test cases of the test suite start.
      // This API supports only one parameter: preset action function.
    });
    beforeEach(() => {
      // Presets an action, which is performed before each unit test case starts.
      // The number of execution times is the same as the number of test cases defined by **it**.
      // This API supports only one parameter: preset action function.
    });
    afterEach(() => {
      // Presets a clear action, which is performed after each unit test case ends.
      // The number of execution times is the same as the number of test cases defined by **it**.
      // This API supports only one parameter: clear action function.
    });
    afterAll(() => {
      // Presets a clear action, which is performed after all test cases of the test suite end.
      // This API supports only one parameter: clear action function.
    });
    it('assertContain', 0, () => {
      // Defines a test case. This API supports three parameters: test case name, filter parameter, and test case function.
      let a = 'abc';
      let b = 'b';
      // Defines a variety of assertion methods, which are used to declare expected boolean conditions.
      expect(a).assertContain(b);
      expect(a).assertEqual(a);
    });
  });
}
"#;

pub fn hvigor_config(sdk: &Sdk) -> String {
    let model = &sdk.model;
    format!(
        r#"{{
  "modelVersion": "{model}",
  "dependencies": {{
  }},
  "execution": {{
    // "analyze": "normal",                     /* Define the build analyze mode. Value: [ "normal" | "advanced" | "ultrafine" | false ]. Default: "normal" */
    // "daemon": true,                          /* Enable daemon compilation. Value: [ true | false ]. Default: true */
    // "incremental": true,                     /* Enable incremental compilation. Value: [ true | false ]. Default: true */
    // "parallel": true,                        /* Enable parallel compilation. Value: [ true | false ]. Default: true */
    // "typeCheck": false,                      /* Enable typeCheck. Value: [ true | false ]. Default: false */
    // "optimizationStrategy": "memory"         /* Define the optimization strategy. Value: [ "memory" | "performance" ]. Default: "memory" */
  }},
  "logging": {{
    // "level": "info"                          /* Define the log level. Value: [ "debug" | "info" | "warn" | "error" ]. Default: "info" */
  }},
  "debugging": {{
    // "stacktrace": false                      /* Disable stacktrace compilation. Value: [ true | false ]. Default: false */
  }},
  "nodeOptions": {{
    // "maxOldSpaceSize": 8192                  /* Enable nodeOptions maxOldSpaceSize compilation. Unit M. Used for the daemon process. Default: 8192*/
    // "exposeGC": true                         /* Enable to trigger garbage collection explicitly. Default: true*/
  }}
}}
"#
    )
}

pub fn root_oh_package(sdk: &Sdk) -> String {
    let model = &sdk.model;
    let hypium = sdk.hypium;
    format!(
        r#"{{
  "modelVersion": "{model}",
  "description": "Please describe the basic information.",
  "dependencies": {{
  }},
  "devDependencies": {{
    "@ohos/hypium": "{hypium}",
    "@ohos/hamock": "1.0.0"
  }}
}}
"#
    )
}

pub fn app_json5(prepared: &Prepared) -> String {
    let bundle = &prepared.bundle_name;
    let icon = if prepared.sdk.uses_layered_icon() {
        "$media:layered_image"
    } else {
        "$media:app_icon"
    };
    format!(
        r#"{{
  "app": {{
    "bundleName": "{bundle}",
    "vendor": "example",
    "versionCode": 1000000,
    "versionName": "1.0.0",
    "buildVersion": "1",
    "icon": "{icon}",
    "label": "$string:app_name"
  }}
}}
"#
    )
}

pub fn app_string_json(name: &str) -> String {
    // Project names are free-form; let serde produce the quoted, escaped JSON
    // string so quotes/backslashes cannot break the manifest.
    let label = serde_json::to_string(name).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"{{
  "string": [
    {{
      "name": "app_name",
      "value": {label}
    }}
  ]
}}
"#
    )
}

pub fn root_build_profile(prepared: &Prepared) -> String {
    let version = &prepared.sdk.version;
    let normalized = prepared.sdk.normalizes_ohm_url();
    let module = &prepared.module_name;
    let modules = match prepared.kind {
        InitKind::Project | InitKind::Hsp => format!(
            r#"    {{
      "name": "{module}",
      "srcPath": "./{module}",
      "targets": [
        {{
          "name": "default",
          "applyToProducts": [
            "default"
          ]
        }}
      ]
    }}"#
        ),
        InitKind::Har => format!(
            r#"    {{
      "name": "{module}",
      "srcPath": "./{module}"
    }}"#
        ),
        InitKind::Package => unreachable!(),
    };

    format!(
        r#"{{
  "app": {{
    "signingConfigs": [],
    "products": [
      {{
        "name": "default",
        "signingConfig": "default",
        "targetSdkVersion": "{version}",
        "compatibleSdkVersion": "{version}",
        "runtimeOS": "HarmonyOS",
        "buildOption": {{
          "strictMode": {{
            "caseSensitiveCheck": true,
            "useNormalizedOHMUrl": {normalized}
          }}
        }}
      }}
    ],
    "buildModeSet": [
      {{
        "name": "debug",
      }},
      {{
        "name": "release"
      }}
    ]
  }},
  "modules": [
{modules}
  ]
}}
"#
    )
}

pub fn hap_build_profile() -> String {
    r#"{
  "apiType": "stageMode",
  "buildOption": {
    "resOptions": {
      "copyCodeResource": {
        "enable": false
      }
    }
  },
  "buildOptionSet": [
    {
      "name": "release",
      "arkOptions": {
        "obfuscation": {
          "ruleOptions": {
            "enable": false,
            "files": [
              "./obfuscation-rules.txt"
            ]
          }
        }
      }
    }
  ],
  "targets": [
    {
      "name": "default"
    },
    {
      "name": "ohosTest"
    }
  ]
}
"#
    .to_string()
}

pub fn library_build_profile() -> String {
    r#"{
  "apiType": "stageMode",
  "buildOption": {
    "resOptions": {
      "copyCodeResource": {
        "enable": false
      }
    }
  },
  "buildOptionSet": [
    {
      "name": "release",
      "arkOptions": {
        "obfuscation": {
          "ruleOptions": {
            "enable": false,
            "files": [
              "./obfuscation-rules.txt"
            ]
          },
          "consumerFiles": [
            "./consumer-rules.txt"
          ]
        }
      }
    }
  ],
  "targets": [
    {
      "name": "default"
    },
    {
      "name": "ohosTest"
    }
  ]
}
"#
    .to_string()
}

pub fn entry_module_json5(sdk: &Sdk) -> String {
    let icon = if sdk.uses_layered_icon() {
        "$media:layered_image"
    } else {
        "$media:icon"
    };
    format!(
        r#"{{
  "module": {{
    "name": "entry",
    "type": "entry",
    "description": "$string:module_desc",
    "mainElement": "EntryAbility",
    "deviceTypes": [
      "phone"
    ],
    "deliveryWithInstall": true,
    "installationFree": false,
    "pages": "$profile:main_pages",
    "abilities": [
      {{
        "name": "EntryAbility",
        "srcEntry": "./ets/entryability/EntryAbility.ets",
        "description": "$string:EntryAbility_desc",
        "icon": "{icon}",
        "label": "$string:EntryAbility_label",
        "startWindowIcon": "$media:startIcon",
        "startWindowBackground": "$color:start_window_background",
        "exported": true,
        "skills": [
          {{
            "entities": [
              "entity.system.home"
            ],
            "actions": [
              "ohos.want.action.home"
            ]
          }}
        ]
      }}
    ],
    "extensionAbilities": [
      {{
        "name": "EntryBackupAbility",
        "srcEntry": "./ets/entrybackupability/EntryBackupAbility.ets",
        "type": "backup",
        "exported": false,
        "metadata": [
          {{
            "name": "ohos.extension.backup",
            "resource": "$profile:backup_config"
          }}
        ]
      }}
    ]
  }}
}}
"#
    )
}

pub fn har_module_json5(module: &str) -> String {
    format!(
        r#"{{
  "module": {{
    "name": "{module}",
    "type": "har",
    "deviceTypes": [
      "default"
    ]
  }}
}}
"#
    )
}

pub fn hsp_module_json5(module: &str) -> String {
    format!(
        r#"{{
  "module": {{
    "name": "{module}",
    "type": "shared",
    "description": "$string:shared_desc",
    "deviceTypes": [
      "phone",
      "tablet",
      "2in1"
    ],
    "deliveryWithInstall": true,
    "pages": "$profile:main_pages"
  }}
}}
"#
    )
}

pub fn har_oh_package(name: &str) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "version": "1.0.0",
  "description": "Please describe the basic information.",
  "main": "Index.ets",
  "author": "",
  "license": "Apache-2.0",
  "dependencies": {{}}
}}
"#
    )
}

pub fn hsp_oh_package(name: &str) -> String {
    har_oh_package(name)
}

pub fn entry_ability(sdk: &Sdk) -> String {
    let color_mode = if sdk.api >= 12 {
        r#"    try {
      this.context.getApplicationContext().setColorMode(ConfigurationConstant.ColorMode.COLOR_MODE_NOT_SET);
    } catch (err) {
      hilog.error(DOMAIN, 'testTag', 'Failed to set colorMode. Cause: %{public}s', JSON.stringify(err));
    }
"#
    } else {
        ""
    };
    let import = if sdk.api >= 12 {
        "import { AbilityConstant, ConfigurationConstant, UIAbility, Want } from '@kit.AbilityKit';"
    } else {
        "import { AbilityConstant, UIAbility, Want } from '@kit.AbilityKit';"
    };

    format!(
        r#"{import}
import {{ hilog }} from '@kit.PerformanceAnalysisKit';
import {{ window }} from '@kit.ArkUI';

const DOMAIN = 0x0000;

export default class EntryAbility extends UIAbility {{
  onCreate(want: Want, launchParam: AbilityConstant.LaunchParam): void {{
    hilog.info(DOMAIN, 'testTag', '%{{public}}s', 'Ability onCreate');
  }}

  onDestroy(): void {{
    hilog.info(DOMAIN, 'testTag', '%{{public}}s', 'Ability onDestroy');
  }}

  onWindowStageCreate(windowStage: window.WindowStage): void {{
    // Main window is created, set main page for this ability
    hilog.info(DOMAIN, 'testTag', '%{{public}}s', 'Ability onWindowStageCreate');

    windowStage.loadContent('pages/Index', (err) => {{
      if (err.code) {{
        hilog.error(DOMAIN, 'testTag', 'Failed to load the content. Cause: %{{public}}s', JSON.stringify(err));
        return;
      }}
      hilog.info(DOMAIN, 'testTag', 'Succeeded in loading the content.');
    }});

{color_mode}  }}

  onWindowStageDestroy(): void {{
    // Main window is destroyed, release UI related resources
    hilog.info(DOMAIN, 'testTag', '%{{public}}s', 'Ability onWindowStageDestroy');
  }}

  onForeground(): void {{
    // Ability has brought to foreground
    hilog.info(DOMAIN, 'testTag', '%{{public}}s', 'Ability onForeground');
  }}

  onBackground(): void {{
    // Ability has back to background
    hilog.info(DOMAIN, 'testTag', '%{{public}}s', 'Ability onBackground');
  }}
}}
"#
    )
}

pub fn index_page(sdk: &Sdk) -> String {
    let body = if sdk.api >= 11 {
        r#"    RelativeContainer() {
      Text(this.message)
        .id('HelloWorld')
        .fontSize($r('app.float.page_text_font_size'))
        .fontWeight(FontWeight.Bold)
        .alignRules({
          center: { anchor: '__container__', align: VerticalAlign.Center },
          middle: { anchor: '__container__', align: HorizontalAlign.Center }
        })
        .onClick(() => {
          this.message = 'Welcome';
        })
    }
    .height('100%')
    .width('100%')
"#
    } else {
        r#"    Row() {
      Column() {
        Text(this.message)
          .fontSize($r('app.float.page_text_font_size'))
          .fontWeight(FontWeight.Bold)
          .onClick(() => {
            this.message = 'Welcome';
          })
      }
      .width('100%')
    }
    .height('100%')
"#
    };
    format!(
        r#"@Entry
@Component
struct Index {{
  @State message: string = 'Hello World';

  build() {{
{body}  }}
}}
"#
    )
}

pub fn entry_resources(sdk: &Sdk) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = vec![
        (
            "base/element/string.json".to_string(),
            r#"{
  "string": [
    {
      "name": "module_desc",
      "value": "module description"
    },
    {
      "name": "EntryAbility_desc",
      "value": "description"
    },
    {
      "name": "EntryAbility_label",
      "value": "label"
    }
  ]
}
"#
            .as_bytes()
            .to_vec(),
        ),
        (
            "base/element/color.json".to_string(),
            r##"{
  "color": [
    {
      "name": "start_window_background",
      "value": "#FFFFFF"
    }
  ]
}
"##
            .as_bytes()
            .to_vec(),
        ),
        (
            "base/element/float.json".to_string(),
            FLOAT_JSON.as_bytes().to_vec(),
        ),
        (
            "base/profile/main_pages.json".to_string(),
            MAIN_PAGES.as_bytes().to_vec(),
        ),
        (
            "base/profile/backup_config.json".to_string(),
            r#"{
  "allowToBackupRestore": true
}
"#
            .as_bytes()
            .to_vec(),
        ),
        ("base/media/startIcon.png".to_string(), png::start_icon()),
    ];

    if sdk.uses_layered_icon() {
        files.push(("base/media/background.png".to_string(), png::background()));
        files.push(("base/media/foreground.png".to_string(), png::foreground()));
        files.push((
            "base/media/layered_image.json".to_string(),
            LAYERED_IMAGE.as_bytes().to_vec(),
        ));
    } else {
        files.push(("base/media/icon.png".to_string(), png::app_icon()));
    }

    if sdk.api >= 12 {
        files.push((
            "dark/element/color.json".to_string(),
            r##"{
  "color": [
    {
      "name": "start_window_background",
      "value": "#000000"
    }
  ]
}
"##
            .as_bytes()
            .to_vec(),
        ));
    }

    files
}

pub fn test_module_json5(module: &str, device_types: &str) -> String {
    let device_types = device_types
        .split(',')
        .map(|device| format!("      \"{}\"", device.trim()))
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        r#"{{
  "module": {{
    "name": "{module}_test",
    "type": "feature",
    "deviceTypes": [
{device_types}
    ],
    "deliveryWithInstall": true,
    "installationFree": false
  }}
}}
"#
    )
}
