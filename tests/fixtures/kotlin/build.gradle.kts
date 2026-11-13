plugins {
    kotlin("jvm") version "1.9.0"
}

val customVersion = "1.0"

dependencies {
    implementation(project(":core"))
    testImplementation(kotlin("test"))
}

tasks.register("hello") {
    doLast { println(customVersion) }
}
