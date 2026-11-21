use serde::{Deserialize, Serialize};

/// Compact 1-byte voxel material identifier.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VoxelId {
    #[default]
    Air = 0,
    Bedrock = 1,
    CaveStone = 2,
    CrackedRock = 3,
    Timber = 4,
    Steel = 5,
    AcousticFoam = 6,
    BiolumCrystal = 7,
    CaveDirt = 8,
}

impl VoxelId {
    #[inline]
    pub const fn is_air(self) -> bool {
        matches!(self, VoxelId::Air)
    }

    #[inline]
    pub const fn is_solid(self) -> bool {
        !self.is_air()
    }

    #[inline]
    pub const fn def(self) -> &'static MaterialDef {
        &MATERIAL_REGISTRY[self as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destructibility {
    /// Unbreakable structural voxel (bedrock, elevator steel, shed frame).
    Unbreakable,
    /// Soft/cracked cave rock or ruin wall breakable with pickaxe/charges.
    BreakableCracked,
    /// Player-placed acoustic foam or barricade (easily removed/broken).
    PlaceableTool,
}

/// Physical 3-band acoustic properties [Low ~250 Hz, Mid ~1 kHz, High ~4 kHz].
#[derive(Clone, Copy, Debug)]
pub struct AcousticMaterial {
    /// Transmission loss in dB per meter of solid material traversed.
    /// Higher high-band loss produces natural muffled sound behind walls.
    pub transmission_loss_db_per_m: [f32; 3],
    /// Specular+diffuse reflection energy multiplier [0.0..1.0] per bounce.
    pub reflection_coeff: [f32; 3],
    /// Surface scattering: 0.0 = mirror-like, 1.0 = diffuse Lambertian scatter.
    pub scattering: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct MaterialDef {
    pub id: VoxelId,
    pub name: &'static str,
    pub color: [f32; 3],
    pub emissive: [f32; 3],
    pub destructibility: Destructibility,
    pub acoustic: AcousticMaterial,
    /// Hardness (hits / seconds to break for breakable materials).
    pub hardness: f32,
}

pub const MATERIAL_REGISTRY: [MaterialDef; 9] = [
    // 0: Air
    MaterialDef {
        id: VoxelId::Air,
        name: "Air",
        color: [0.0, 0.0, 0.0],
        emissive: [0.0, 0.0, 0.0],
        destructibility: Destructibility::Unbreakable,
        acoustic: AcousticMaterial {
            transmission_loss_db_per_m: [0.0, 0.0, 0.0],
            reflection_coeff: [0.0, 0.0, 0.0],
            scattering: 0.0,
        },
        hardness: 0.0,
    },
    // 1: Bedrock
    MaterialDef {
        id: VoxelId::Bedrock,
        name: "Bedrock",
        color: [0.10, 0.11, 0.13],
        emissive: [0.0, 0.0, 0.0],
        destructibility: Destructibility::Unbreakable,
        acoustic: AcousticMaterial {
            transmission_loss_db_per_m: [12.0, 24.0, 42.0],
            reflection_coeff: [0.90, 0.85, 0.78],
            scattering: 0.45,
        },
        hardness: f32::INFINITY,
    },
    // 2: CaveStone
    MaterialDef {
        id: VoxelId::CaveStone,
        name: "Cave Stone",
        color: [0.24, 0.25, 0.28],
        emissive: [0.0, 0.0, 0.0],
        destructibility: Destructibility::Unbreakable,
        acoustic: AcousticMaterial {
            transmission_loss_db_per_m: [8.0, 18.0, 32.0],
            reflection_coeff: [0.86, 0.82, 0.72],
            scattering: 0.55,
        },
        hardness: f32::INFINITY,
    },
    // 3: CrackedRock (breakable shortcuts!)
    MaterialDef {
        id: VoxelId::CrackedRock,
        name: "Cracked Rock",
        color: [0.42, 0.34, 0.26],
        emissive: [0.0, 0.0, 0.0],
        destructibility: Destructibility::BreakableCracked,
        acoustic: AcousticMaterial {
            transmission_loss_db_per_m: [5.0, 12.0, 22.0],
            reflection_coeff: [0.72, 0.65, 0.55],
            scattering: 0.70,
        },
        hardness: 1.5,
    },
    // 4: Timber (warm shed wood)
    MaterialDef {
        id: VoxelId::Timber,
        name: "Timber",
        color: [0.46, 0.31, 0.19],
        emissive: [0.0, 0.0, 0.0],
        destructibility: Destructibility::Unbreakable,
        acoustic: AcousticMaterial {
            transmission_loss_db_per_m: [4.0, 10.0, 18.0],
            reflection_coeff: [0.45, 0.38, 0.32],
            scattering: 0.60,
        },
        hardness: f32::INFINITY,
    },
    // 5: Steel (elevator shaft)
    MaterialDef {
        id: VoxelId::Steel,
        name: "Shaft Steel",
        color: [0.30, 0.34, 0.38],
        emissive: [0.0, 0.0, 0.0],
        destructibility: Destructibility::Unbreakable,
        acoustic: AcousticMaterial {
            transmission_loss_db_per_m: [14.0, 24.0, 38.0],
            reflection_coeff: [0.94, 0.92, 0.88],
            scattering: 0.20,
        },
        hardness: f32::INFINITY,
    },
    // 6: AcousticFoam (placeable sound-deadening barrier!)
    MaterialDef {
        id: VoxelId::AcousticFoam,
        name: "Acoustic Foam",
        color: [0.78, 0.68, 0.22],
        emissive: [0.0, 0.0, 0.0],
        destructibility: Destructibility::PlaceableTool,
        acoustic: AcousticMaterial {
            transmission_loss_db_per_m: [7.0, 24.0, 44.0],
            reflection_coeff: [0.16, 0.08, 0.05],
            scattering: 0.92,
        },
        hardness: 0.5,
    },
    // 7: BiolumCrystal (atmospheric glowing cave landmark)
    MaterialDef {
        id: VoxelId::BiolumCrystal,
        name: "Bioluminescent Crystal",
        color: [0.18, 0.85, 0.78],
        emissive: [0.15, 0.90, 0.82],
        destructibility: Destructibility::BreakableCracked,
        acoustic: AcousticMaterial {
            transmission_loss_db_per_m: [10.0, 20.0, 30.0],
            reflection_coeff: [0.90, 0.92, 0.94],
            scattering: 0.15,
        },
        hardness: 1.0,
    },
    // 8: CaveDirt (soft echoing floor)
    MaterialDef {
        id: VoxelId::CaveDirt,
        name: "Cave Dirt",
        color: [0.27, 0.22, 0.18],
        emissive: [0.0, 0.0, 0.0],
        destructibility: Destructibility::Unbreakable,
        acoustic: AcousticMaterial {
            transmission_loss_db_per_m: [6.0, 15.0, 28.0],
            reflection_coeff: [0.55, 0.45, 0.35],
            scattering: 0.85,
        },
        hardness: f32::INFINITY,
    },
];
