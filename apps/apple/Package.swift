// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "HortusApple",
    platforms: [.macOS("26.0"), .iOS("18.0")],
    products: [
        .library(name: "HortusCore", targets: ["HortusCore"]),
        .library(name: "HortusApple", targets: ["HortusApple"]),
    ],
    targets: [
        .systemLibrary(name: "HortusFFI", path: "Generated/HortusFFI"),
        .target(name: "HortusCore", dependencies: ["HortusFFI"], path: "Generated/HortusCore"),
        .target(name: "HortusApple", dependencies: ["HortusCore"]),
    ]
)
