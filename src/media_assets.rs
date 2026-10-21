// SHA-256 and byte counts pinned at development time from upstream GitHub assets.
// https://github.com/eugeneware/ffmpeg-static/releases/tag/b6.1.1
// Windows ia32 uses b6.0 with measured download hashes and its Windows license.
// Never fetch replacement checksum metadata at runtime.
pub const MEDIA_ASSET_BASE: &str = "https://github.com/eugeneware/ffmpeg-static/releases/download";

#[derive(Debug)]
pub struct Asset {
    pub name: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    pub gzip: Option<&'static Asset>,
}

#[derive(Debug)]
pub struct MediaPlan {
    pub release: &'static str,
    pub ffmpeg: Asset,
    pub ffprobe: Asset,
    pub license: Asset,
    pub readme: Asset,
}

macro_rules! asset {
    ($name:expr, $sha:expr, $size:expr) => {
        Asset { name: $name, sha256: $sha, size: $size, gzip: None }
    };
    ($name:expr, $sha:expr, $size:expr, $gz:expr, $gzsha:expr, $gzsize:expr) => {
        Asset { name: $name, sha256: $sha, size: $size, gzip: Some(&Asset { name: $gz, sha256: $gzsha, size: $gzsize, gzip: None }) }
    };
}

const LINUX_LICENSE_SHA: &str = "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903";

pub static MEDIA_ASSETS: &[(&str, MediaPlan)] = &[
    (
        "win32-x64",
        MediaPlan {
            release: "b6.1.1",
            ffmpeg: asset!(
                "ffmpeg-win32-x64",
                "04e1307997530f9cf2fe35cba2ca7e8875ca91da02f89d6c7243df819c94ad00",
                82797568,
                "ffmpeg-win32-x64.gz",
                "8883a3dffbd0a16cf4ef95206ea05283f78908dbfb118f73c83f4951dcc06d77",
                29581307
            ),
            ffprobe: asset!(
                "ffprobe-win32-x64",
                "3a7e2dc003dc2cd1472827e4c7c4f056ae1ae0ae7c5bbc580c99b49827351ba4",
                82668032,
                "ffprobe-win32-x64.gz",
                "f309e6223ad89d2fe54bccd420a7709b66fd27540674e92309578ed491a43c8d",
                29521644
            ),
            license: asset!("win32-x64.LICENSE", LINUX_LICENSE_SHA, 35147),
            readme: asset!("win32-x64.README", "a636a7183c58006351acbaf35303c0ed85c6e1320fd4e80de453ba6157de6311", 39494),
        },
    ),
    (
        "darwin-x64",
        MediaPlan {
            release: "b6.1.1",
            ffmpeg: asset!(
                "ffmpeg-darwin-x64",
                "ebdddc936f61e14049a2d4b549a412b8a40deeff6540e58a9f2a2da9e6b18894",
                78862176,
                "ffmpeg-darwin-x64.gz",
                "929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106",
                25296431
            ),
            ffprobe: asset!(
                "ffprobe-darwin-x64",
                "fa3add0ce901f7241abe0dfc0155d958fc834aca3f8ce61f87cc712ae669c1e0",
                78780408,
                "ffprobe-darwin-x64.gz",
                "d4da574d6e2e197bd259b47d69cf262df9e312af24ad960444f6d806d3d4c186",
                25239438
            ),
            license: asset!("darwin-x64.LICENSE", "2e1d16c72fd74e12063776371da757322f8b77589386532f4fd8634bde7de1af", 4346),
            readme: asset!("darwin-x64.README", "e88a0325f8e5b75210355e37341824f074d3cd82def2125be54c914b62848a36", 6227),
        },
    ),
    (
        "darwin-arm64",
        MediaPlan {
            release: "b6.1.1",
            ffmpeg: asset!(
                "ffmpeg-darwin-arm64",
                "a90e3db6a3fd35f6074b013f948b1aa45b31c6375489d39e572bea3f18336584",
                45568216,
                "ffmpeg-darwin-arm64.gz",
                "8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa",
                19246198
            ),
            ffprobe: asset!(
                "ffprobe-darwin-arm64",
                "bb2db6f5d8cef919da12fbf592119a987202a8c060a886f3cab091f9cab90b64",
                45528808,
                "ffprobe-darwin-arm64.gz",
                "d986a8ec7b030899fe66a8a288ed809a3543338705a3ce178cfb85869c5d80be",
                19207077
            ),
            license: asset!("darwin-arm64.LICENSE", "cb48bf09a11f5fb576cddb0431c8f5ed0a60157a9ec942adffc13907cbe083f2", 4376),
            readme: asset!("darwin-arm64.README", "05ba4b92c96605434b1aaae3eedf5a2c280c9607bf78ffca9a5b536d9af2dc6a", 1810),
        },
    ),
    (
        "linux-x64",
        MediaPlan {
            release: "b6.1.1",
            ffmpeg: asset!(
                "ffmpeg-linux-x64",
                "e7e7fb30477f717e6f55f9180a70386c62677ef8a4d4d1a5d948f4098aa3eb99",
                79826272,
                "ffmpeg-linux-x64.gz",
                "bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa",
                29354986
            ),
            ffprobe: asset!(
                "ffprobe-linux-x64",
                "4f231a1960d83e403d08f7971e271707bec278a9ae18e21b8b5b03186668450d",
                79665792,
                "ffprobe-linux-x64.gz",
                "25d9b6ccb05e3d9de9e04e31e2506d8dd7f9f0418981965ac6df12e8d3afd067",
                29276839
            ),
            license: asset!("linux-x64.LICENSE", LINUX_LICENSE_SHA, 35147),
            readme: asset!("linux-x64.README", "72f4b1b06d419d22ace6e7cc75f06826f90737345aa0b1736158929f4aacc537", 2235),
        },
    ),
    (
        "linux-ia32",
        MediaPlan {
            release: "b6.1.1",
            ffmpeg: asset!(
                "ffmpeg-linux-ia32",
                "c6472eb993612db72ca50893a34137ba11173e60a1a4c028d4660a3f755d2490",
                52155436,
                "ffmpeg-linux-ia32.gz",
                "169b27c078a8ecedb814cac67afccf15a9868d63e9d74ef86088adefaa500d00",
                22184031
            ),
            ffprobe: asset!(
                "ffprobe-linux-ia32",
                "a13b4cce0a4b9bd6714672e4df81076ec06f4c295d798162d48e3de68c781d6d",
                51955948,
                "ffprobe-linux-ia32.gz",
                "a75c55bcaad1b0e79f2201d82b9cc43903d950857615f91baaea5ce92d756e63",
                22093495
            ),
            license: asset!("linux-ia32.LICENSE", LINUX_LICENSE_SHA, 35147),
            readme: asset!("linux-ia32.README", "3bbce64eba997007b3fd5324bb105f10ab6b0a499796c7d34b844b51fda0528b", 2209),
        },
    ),
    (
        "linux-arm64",
        MediaPlan {
            release: "b6.1.1",
            ffmpeg: asset!(
                "ffmpeg-linux-arm64",
                "6bb182d0d75d23028db82e9e4f723ca69b853d055698486e6984ddb2c06fb8ce",
                51134160,
                "ffmpeg-linux-arm64.gz",
                "754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0",
                25568691
            ),
            ffprobe: asset!(
                "ffprobe-linux-arm64",
                "d17ae9b4c297d48e2521ba14e417bb0537c6ff77c584cdbcd6bb0d8d0307a2e8",
                50994160,
                "ffprobe-linux-arm64.gz",
                "2ab6aba60ee84412dff9188720703376cb4e7aaf7e0b5e43aa8249f2acae5bf8",
                25493573
            ),
            license: asset!("linux-arm64.LICENSE", LINUX_LICENSE_SHA, 35147),
            readme: asset!("linux-arm64.README", "d6777d2fd276b23f0ac6666fa619e88ffe4826521881c7ff83836e30cb4acec2", 2217),
        },
    ),
    (
        "linux-arm",
        MediaPlan {
            release: "b6.1.1",
            ffmpeg: asset!(
                "ffmpeg-linux-arm",
                "0afba4a11110e6e402053e0fc14c33a7eb207d7a588688ae87dba471a0f06c71",
                31793580,
                "ffmpeg-linux-arm.gz",
                "64b115a12f0ab77c277e3c418aae8b40ef881e75e746a0e2d066a206b9bc5172",
                19167563
            ),
            ffprobe: asset!(
                "ffprobe-linux-arm",
                "e61ef24870ddc3b80aa8b435512a545bd889e4ed4c2fc7c885f7d3dabad2a53f",
                31671692,
                "ffprobe-linux-arm.gz",
                "2471169c19fea00018413eebf188703c19ae5ab614477465146d4cdc7458b55d",
                19084901
            ),
            license: asset!("linux-arm.LICENSE", LINUX_LICENSE_SHA, 35147),
            readme: asset!("linux-arm.README", "faaea83a6d92419cb112d5de5ac673f51b72401830e068c4d003b3229ee1c89a", 2178),
        },
    ),
    (
        "win32-ia32",
        MediaPlan {
            release: "b6.0",
            ffmpeg: asset!(
                "ffmpeg-win32-ia32",
                "fb3766af5cc193ca863e15cd4554a33732973209dad5e3c1433b5e291bceb16c",
                74250240,
                "ffmpeg-win32-ia32.gz",
                "af47ca5de7b9f859f48e4e96ea831bf551b100144b0b871762ab48fba1152f56",
                31390495
            ),
            ffprobe: asset!(
                "ffprobe-win32-ia32",
                "c1ca7f2c0f93285629360d5afe622080ec4ce53407c427f7b8a48d7e8fcab02d",
                74125312,
                "ffprobe-win32-ia32.gz",
                "9ccc2eb5c37f8ba8c4cf8f0a73ecb508b740fa64b0a2c3171acc98f89f409e87",
                31332907
            ),
            license: asset!("win32-x64.LICENSE", LINUX_LICENSE_SHA, 35147),
            readme: asset!("win32-x64.README", "8cf2907e089d95142c752a69cd934956b552e71d8d4762e65dbd227f5e40237e", 38167),
        },
    ),
];

/// Pinned media tools for a platform. Windows 11 ARM runs the x64 tools via emulation.
pub fn media_plan(platform: &str, arch: &str) -> Option<&'static MediaPlan> {
    let arch = if platform == "win32" && arch == "arm64" { "x64" } else { arch };
    let key = format!("{platform}-{arch}");
    MEDIA_ASSETS.iter().find(|(name, _)| *name == key).map(|(_, plan)| plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_cover_desktop_platforms() {
        assert_eq!(media_plan("win32", "arm64").unwrap().ffmpeg.name, "ffmpeg-win32-x64");
        assert_eq!(media_plan("win32", "ia32").unwrap().release, "b6.0");
        assert!(media_plan("android", "arm64").is_none());
        for (_, plan) in MEDIA_ASSETS {
            for asset in [&plan.ffmpeg, &plan.ffprobe, &plan.license, &plan.readme] {
                assert_eq!(asset.sha256.len(), 64);
                assert!(asset.size > 0);
            }
        }
    }
}
