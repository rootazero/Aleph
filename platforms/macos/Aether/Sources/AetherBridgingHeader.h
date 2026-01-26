//
//  AetherBridgingHeader.h
//  Aether
//
//  Bridging header for UniFFI-generated FFI functions and Metal shaders
//

#ifndef AetherBridgingHeader_h
#define AetherBridgingHeader_h

// Import the UniFFI-generated C header
// This provides all the FFI function declarations and type definitions
#import "aetherFFI.h"

// Import Metal shader types for Liquid Glass effect
// This provides shared type definitions between Swift and Metal
#import "ShaderTypes.h"

#endif /* AetherBridgingHeader_h */
