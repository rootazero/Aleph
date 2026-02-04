//
//  main.swift
//  Aleph
//
//  Main entry point for Aleph macOS application.
//
//  IMPORTANT: We use traditional AppKit launch instead of SwiftUI App lifecycle
//  to avoid a macOS 26 (Tahoe) crash. The SwiftUI Settings/WindowGroup scenes
//  trigger NSHostingView.invalidateSafeAreaCornerInsets() crash during early
//  initialization when the view hierarchy is not yet fully set up.
//
//  By using NSApplicationMain(), we defer all UI initialization to
//  AppDelegate.applicationDidFinishLaunching(), where we have full control
//  over the timing of window creation.
//

import Cocoa

// Create and configure the application
let app = NSApplication.shared

// Create the AppDelegate
let delegate = AppDelegate()
app.delegate = delegate

// Run the application
_ = NSApplicationMain(CommandLine.argc, CommandLine.unsafeArgv)
