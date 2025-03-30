import net.fabricmc.loom.api.LoomGradleExtensionAPI

plugins {
    id("fabric-loom") version "1.10.5"
    java
}

repositories {
    maven {
        name = "Fabric"
        url = uri("https://maven.fabricmc.net/")
    }
    maven {
        name = "ParchmentMC"
        url = uri("https://maven.parchmentmc.org")
    }
    mavenCentral()
}

dependencies {
    "minecraft"("com.mojang:minecraft:${project.properties["minecraft_version"]}")
    "mappings"(loom.layered {
       officialMojangMappings()
       parchment("org.parchmentmc.data:parchment-${project.properties["parchment_mc_version"]}:${project.properties["parchment_version"]}")
    })
}

afterEvaluate {
    val genSources = tasks.getByName<net.fabricmc.loom.task.GenerateSourcesTask>("genSourcesWithVineflower")
    // https://github.com/FabricMC/fabric-loom/issues/1117 -- generates bad diffs.
    genSources.useCache = false
}

tasks.register<Sync>("unpackSourcesIntoKnownDir") {
    val genSources = tasks.getByName<net.fabricmc.loom.task.GenerateSourcesTask>("genSourcesWithVineflower")
    inputs.files(genSources.sourcesOutputJar)
    from(zipTree(genSources.sourcesOutputJar))
    into("decompiledSources")
}

tasks.register("exportLibraries") {
    val rootProvider = configurations["minecraftLibraries"].incoming.resolutionResult.rootComponent
    val serverRootProvider = configurations["minecraftServerLibraries"].incoming.resolutionResult.rootComponent
    inputs.property("rootProvider", rootProvider)
    inputs.property("serverRootProvider", serverRootProvider)
    val output = project.layout.buildDirectory.file("libraries.txt")
    outputs.file(output)
    doLast {
        val mergedDependencies = (rootProvider.get().dependencies + serverRootProvider.get().dependencies)
            .asSequence()
            .map { it.requested.displayName }
            .distinct()
            .sorted()
        output.get().asFile.writeText(mergedDependencies.joinToString("\n"))
    }
}

java.toolchain.languageVersion = JavaLanguageVersion.of(21)
