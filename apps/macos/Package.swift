// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Hortus",
    platforms: [.macOS("27.0")],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm.git", exact: "1.99.1"),
    ],
    targets: [
        .systemLibrary(name: "HortusFFI", path: "Generated/HortusFFI"),
        .target(name: "HortusCore", dependencies: ["HortusFFI"], path: "Generated/HortusCore"),
        .executableTarget(name: "Hortus", dependencies: [
            "HortusCore",
            .product(name: "SwiftTerm", package: "SwiftTerm"),
        ]),
        .testTarget(name: "HortusTests", dependencies: ["Hortus"]),
    ]
)
