// swift-tools-version: 5.9
import PackageDescription

let package = Package(name: "Calendar", targets: [
    .target(name: "Calendar"),
    .testTarget(name: "CalendarTests", dependencies: ["Calendar"]),
    .testTarget(name: "DisplayTests", dependencies: ["Calendar"]),
])
