plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The interface: ui/ from the app's own repository at the commit in apps/leaflet.commit
// (the CI checks it out; APPSHELL_LEAFLET_UI says where).
val uiSource = System.getenv("APPSHELL_LEAFLET_UI") ?: "../../build/leaflet/ui"
val copyUi by tasks.registering(Sync::class) {
    from(uiSource)
    into(layout.buildDirectory.dir("generated/ui/ui"))
}

android {
    namespace = "app.leaflet"
    compileSdk = 35
    defaultConfig {
        applicationId = "app.leaflet"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    sourceSets["main"].assets.srcDir(layout.buildDirectory.dir("generated/ui"))
    // The self-test scripts and test files (CI puts them here; see .github/workflows/ci.yml)
    sourceSets["androidTest"].assets.srcDir(System.getenv("APPSHELL_TEST_ASSETS") ?: "../../build/android-test-assets")
    buildTypes {
        release {
            isMinifyEnabled = false
            // Signed with the debug key until there is a release key; that one is
            // yours to make and keep, and never goes in the repository.
            signingConfig = signingConfigs.getByName("debug")
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
