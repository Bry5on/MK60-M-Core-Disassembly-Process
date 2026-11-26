#ifndef CONFIG_H
#define CONFIG_H

// Edit in platformio.ini
#ifndef	PCB_VER
#define PCB_VER 40                  // versions older than v4.0 (40) use SPI for SD card
#endif

// Sd card handle - PCB v3.8 and below use old style SPI SD card interface
#if PCB_VER < 40
	#include <SD.h>
	#define SD_USE SD

	#define SD_CS           15
	#define SD_CS           15
	#define SD_SCK          14
	#define SD_MISO         27
	#define SD_MOSI         13
	SPIClass SDSPI(HSPI);

#else
	#include <SD_MMC.h>
	#define SD_USE SD_MMC
#endif


// Pin definitions
#if PCB_VER <= 40
 #if PCB_VER == 36
	#define I2C_SCL 		33
	#define I2C_SDA 		22

	#define ENC_B 			25
 #else
	#define I2C_SCL 	    25
	#define I2C_SDA 	    22

	#define ENC_B 		    33
 #endif

	#define ENC_A 		    19
	#define ENC_C 		     0

    #define OLED_MOSI 	    23
    #define OLED_CLK 	    18
    #define OLED_DC		    21
    #define OLED_CS		    32
    #define OLED_RESET 	    26
    #define LED_C		    12

    #define ANALOG_1 	    35
    #define ANALOG_2 	    34
    #define ANALOG_3 	    39
    #define ANALOG_4 	    36
    #define ANALOG_5 	    33

    #define CAN_TX		     5
    #define CAN_RX		     4

    #define SERIAL_KLINE_TX	17
	#define SERIAL_KLINE_RX	16
#else
	#define I2C_SCL 	    42
	#define I2C_SDA 	    41

	#define ENC_A 		    12
	#define ENC_B 		    11
	#define ENC_C 		     0
	#define ENC_E 		     3

	#define USB_DN		    19
	#define USB_DP		    20

	#define SD_SCK 		    39
	#define SD_MISO		    40
	#define SD_MOSI		    38

    #define OLED_MOSI 	    48
	#define OLED_CLK 	    47
	#define OLED_DC		    21
	#define OLED_CS		    14
	#define OLED_RESET 	    13
	#define LED_C		    10

	#define ANALOG_0 	     9 // Input voltage
	#define ANALOG_1 	     4
	#define ANALOG_2 	     5
	#define ANALOG_3 	     6
	#define ANALOG_4 	     7
	#define ANALOG_5 	     1
	#define ANALOG_6 	     2
	#define ANALOG_7 	     8

	#define CAN_TX		    18
	#define CAN_RX		    17

    #define LOG_TX		    37
	#define LOG_RX		    36

	#define SERIAL_KLINE_TX	16
	#define SERIAL_KLINE_RX	15

	// ---- Raw SPI-master mode (25LC040 config-EEPROM read/write at 3.3V) ----
	// Locked pin map, verified against the pin defs above and the board wiki header table.
	// These alias existing GPIOs that are free while the SPI EEPROM tool runs:
	//   CS   = GPIO 8  (= ANALOG_7, broken out on the ANALOG2 header pin 3)
	//   MISO = GPIO 11 (= ENC_B,    I2C/GPS header pin 4 "EB")
	//   MOSI = GPIO 41 (= I2C_SDA,  I2C/GPS header pin 1)
	//   SCK  = GPIO 42 (= I2C_SCL,  I2C/GPS header pin 2)
	// Chip power is the I2C/GPS header 3V3 + GND (not driven by firmware). 3V3-native: the
	// whole point of using Gauge.S over a 5V CH341A.
	#define SPI_CS		     8
	#define SPI_MISO	    11
	#define SPI_MOSI	    41
	#define SPI_SCK		    42

	// ---- Relay driver + ignition feedback (diagnostic-mode ignition-timing rig) ----
	// EE (ANALOG2 header pin 4) = N-ch MOSFET gate: drive HIGH = MOSFET on = sinks the relay IN low =
	//   relay ON (relay module jumper in "L"). GPIO 3 is an S3 strapping pin, so it's driven LOW from
	//   setup() to minimise the boot blip.
	// A5 (ANALOG2 header pin 1) = 12V-tolerant input (10k/2.2k divider) = ignition feedback. GPIO 1 =
	//   ADC1_CH0; line V = ADC_mV * (10+2.2)/2.2 ~= ADC_mV * 5.545.
	#define RELAY_EE	 ENC_E     // GPIO 3
	#define FEEDBACK_A5	 ANALOG_5  // GPIO 1
	// Board dividers: the firmware converts the raw ADC pin millivolts to the ACTUAL line millivolts before
	// sending EVT_ADC / EVT_12_V_IN, so the host never needs to know the hardware. A5 is the switched-12V
	// feedback; ANALOG_0 is the permanent main-connector 12V input (calibrated vs a multimeter: 2153 mV pin
	// = 12.23 V -> 5.68).
	#define FEEDBACK_A5_DIV  5.545f
	#define V12_IN_DIV       5.68f
	// Relay safety: if the relay is ON and the main 12V input falls below this (coil can't pull in -> the
	// gate chatters/buzzes), the firmware drives the relay OFF on its own. In line millivolts.
	#define RELAY_MIN_12V_MV 7000
#endif

// Debug print levels
enum DebugLevel : uint8_t {
	DEBUG_INFO = 0,
	DEBUG_VERBOSE_INFO,
	DEBUG_WARNING,
	DEBUG_ERROR,
	DEBUG_MAX = 255
};

// ---- K-line wire format ----
// The E46 MK60 DSC speaks BMW DS2 at 9600 8E1 (even parity) - confirmed on the bench 2026-11-11.
// 8N1 does NOT work (the module rejects every byte on a parity error). Fixed here, not host-selectable.
#define KLINE_SERIAL_FORMAT SERIAL_8E1

#endif // CONFIG_H