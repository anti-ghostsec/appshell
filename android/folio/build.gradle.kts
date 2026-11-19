plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The interface: ui/ from the app's own repository at the commit in apps/folio.commit
// (the CI checks it out; APPSHELL_FOLIO_UI says where).
val uiSource = System.getenv("APPSHELL_FOLIO_UI") ?: "../../build/folio/ui"
val copyUi by tasks.registering(Sync::class) {
    from(uiSource)
    into(layout.buildDirectory.dir("generated/ui/ui"))
}

android {
    namespace = "app.folio"
    compileSdk = 35
    defaultConfig {
        applicationId = "app.folio"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    sourceSets["main"].assets.srcDir(layout.buildDirectory.dir("generated/ui"))
    // The self-test scripts and test files (CI puts them here; see .github/workflows/ci.yml)
    sourceSets["androidTest"].assets.srcDir(System.getenv("APPSHELL_TEST_ASSETS") ?: "../../build/android-test-assets")
    // The release key: a keystore the release workflow writes from the
    // repository's secrets (see .github/workflows/release.yml). It never goes
    // in the repository. Without it, release builds (the CI's test builds, a
    // local build) are signed with the debug key, and release.yml refuses to
    // publish those.
    val releaseKeystore = System.getenv("APPSHELL_KEYSTORE")?.takeIf { it.isNotBlank() }
    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                storeFile = file(releaseKeystore)
                storePassword = System.getenv("APPSHELL_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("APPSHELL_KEY_ALIAS")
                keyPassword = System.getenv("APPSHELL_KEY_PASSWORD")
            }
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName(if (releaseKeystore != null) "release" else "debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    packaging {
        jniLibs.useLegacyPackaging = false
    }
}

tasks.named("preBuild") { dependsOn(copyUi) }

dependencies {
    implementation(project(":shell"))
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test:core-ktx:1.6.1")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
}
