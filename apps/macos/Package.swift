// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Hortus",
    platforms: [.macOS("27.0")],
    dependencies: [
        .package(path: "../apple"),
        .package(url: "https://github.com/migueldeicaza/SwiftTerm.git", exact: "1.99.1"),
    ],
    targets: [
        .executableTarget(name: "Hortus", dependencies: [
            .product(name: "HortusCore", package: "apple"),
            .product(name: "HortusApple", package: "apple"),
            .product(name: "SwiftTerm", package: "SwiftTerm"),
        ]),
        .testTarget(name: "HortusTests", dependencies: ["Hortus"]),
    ]
)
