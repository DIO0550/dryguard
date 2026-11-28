// swift-tools-version: 5.9
import PackageDescription

let package = Package(name: "Shop", targets: [
    .target(name: "Billing"),
    .target(name: "Inventory"),
    .testTarget(name: "BillingTests", dependencies: ["Billing"]),
    .testTarget(name: "InventoryTests", dependencies: ["Inventory"]),
])
